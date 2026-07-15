//! SIP Stack Coordinator.
//!
//! Wires together the transport, transaction, and dialog layers into a
//! running SIP stack. Provides the main event loop that drives message
//! processing from recv through transaction matching to dialog routing.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::dialog::Dialog;
use crate::parser::{
    RequestLine, SipHeader, SipMessage, SipMethod, StartLine, extract_tag, header_names,
};
use crate::session::SipSession;
use crate::transaction::{
    ClientTransaction, NonInviteClientTransaction, NonInviteServerTransaction, ServerTransaction,
};
use crate::transport::{SipTransport, TransportError, UdpTransport};

fn cseq_number(msg: &SipMessage) -> Option<&str> {
    msg.get_header(header_names::CSEQ)?.split_whitespace().next()
}

fn request_identity_matches(original: &SipMessage, retransmission: &SipMessage) -> bool {
    original.call_id() == retransmission.call_id()
        && cseq_number(original) == cseq_number(retransmission)
        && original.from_header().and_then(extract_tag)
            == retransmission.from_header().and_then(extract_tag)
}

/// Build the transaction ACK for a 300-699 INVITE response. It reuses the
/// INVITE branch and Request-URI, unlike the new-branch dialog ACK for 2xx.
fn build_non_2xx_ack(invite: &SipMessage, response: &SipMessage) -> Option<SipMessage> {
    let request_line = match &invite.start_line {
        StartLine::Request(line) => RequestLine {
            method: SipMethod::Ack,
            uri: line.uri.clone(),
            version: line.version.clone(),
        },
        _ => return None,
    };
    let cseq = cseq_number(invite)?;
    let mut headers: Vec<SipHeader> = invite.headers.iter()
        .filter(|header| {
            header.name.eq_ignore_ascii_case(header_names::VIA)
                || header.name.eq_ignore_ascii_case(header_names::MAX_FORWARDS)
                || header.name.eq_ignore_ascii_case(header_names::FROM)
                || header.name.eq_ignore_ascii_case(header_names::CALL_ID)
                || header.name.eq_ignore_ascii_case(header_names::ROUTE)
        })
        .cloned()
        .collect();
    headers.push(SipHeader {
        name: header_names::TO.to_string(),
        value: response.to_header()?.to_string(),
    });
    headers.push(SipHeader {
        name: header_names::CSEQ.to_string(),
        value: format!("{} ACK", cseq),
    });
    headers.push(SipHeader {
        name: header_names::CONTENT_LENGTH.to_string(),
        value: "0".to_string(),
    });
    Some(SipMessage {
        start_line: StartLine::Request(request_line),
        headers,
        body: String::new(),
    })
}

/// Events emitted by the SIP stack for the application layer.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum SipEvent {
    /// A new incoming INVITE (new session).
    IncomingInvite {
        session: SipSession,
        request: SipMessage,
        remote_addr: SocketAddr,
    },
    /// A response was received for an outbound transaction.
    Response {
        response: SipMessage,
        remote_addr: SocketAddr,
    },
    /// A BYE was received (session termination).
    IncomingBye {
        call_id: String,
        request: SipMessage,
        remote_addr: SocketAddr,
    },
    /// A CANCEL was received that terminated a pending INVITE. The stack has
    /// already answered at the transaction layer (200 OK to the CANCEL, 487
    /// Request Terminated to the INVITE, RFC 3261 §9.2); the application
    /// layer must abort the channel and its dialplan execution.
    IncomingCancel {
        call_id: String,
        request: SipMessage,
        remote_addr: SocketAddr,
    },
    /// A 2xx ACK matched as a new-branch dialog request and delivered to TU.
    IncomingAck {
        call_id: String,
        request: SipMessage,
        remote_addr: SocketAddr,
    },
    /// A successful INVITE response exhausted its ACK-wait lifetime.
    InviteAckTimeout {
        call_id: String,
    },
    /// A non-INVITE request was received (OPTIONS, REGISTER, etc.).
    IncomingRequest {
        request: SipMessage,
        remote_addr: SocketAddr,
    },
    /// A transaction timed out.
    TransactionTimeout {
        branch: String,
    },
}

/// Manages active client (INVITE) transactions keyed by branch.
struct CachedInviteFinal {
    request: SipMessage,
    remote_addr: SocketAddr,
    response: SipMessage,
    expires_at: Instant,
}

struct TransactionTimeoutEvent {
    branch: String,
    invite_ack_call_id: Option<String>,
}

/// Exact sizes of the four RFC 3261 transaction maps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransactionCounts {
    pub invite_client: usize,
    pub invite_server: usize,
    pub non_invite_client: usize,
    pub non_invite_server: usize,
}

struct MatchedResponse {
    branch: String,
    transaction_ack: Option<(SipMessage, SocketAddr)>,
}

struct TransactionLayer {
    invite_client_txns: HashMap<String, ClientTransaction>,
    invite_server_txns: HashMap<String, ServerTransaction>,
    non_invite_client_txns: HashMap<String, NonInviteClientTransaction>,
    non_invite_server_txns: HashMap<String, NonInviteServerTransaction>,
    /// TU response cache retained after the server transaction is reaped, so
    /// a late UDP retransmission cannot create a second call.
    invite_final_cache: HashMap<String, CachedInviteFinal>,
}

impl TransactionLayer {
    fn new() -> Self {
        Self {
            invite_client_txns: HashMap::new(),
            invite_server_txns: HashMap::new(),
            non_invite_client_txns: HashMap::new(),
            non_invite_server_txns: HashMap::new(),
            invite_final_cache: HashMap::new(),
        }
    }

    /// Extract the branch parameter from the top Via header.
    fn extract_branch(msg: &SipMessage) -> Option<String> {
        let via = msg.get_header(header_names::VIA)?;
        for param in via.split(';') {
            let param = param.trim();
            if let Some(value) = param.strip_prefix("branch=") {
                return Some(value.to_string());
            }
        }
        None
    }

    /// Route a received response to the matching client transaction.
    fn process_response(
        &mut self,
        response: &SipMessage,
        src: SocketAddr,
    ) -> Option<MatchedResponse> {
        let branch = Self::extract_branch(response)?;

        if let Some(txn) = self.invite_client_txns.get_mut(&branch) {
            if txn.remote_addr != src {
                return None;
            }
            let status = response.status_code().unwrap_or_default();
            let transaction_ack = (300..700).contains(&status)
                .then(|| build_non_2xx_ack(&txn.request, response))
                .flatten()
                .map(|ack| (ack, txn.remote_addr));
            txn.on_response(response.clone());
            return Some(MatchedResponse { branch, transaction_ack });
        }

        if let Some(txn) = self.non_invite_client_txns.get_mut(&branch) {
            if txn.remote_addr != src {
                return None;
            }
            txn.on_response(response.clone());
            return Some(MatchedResponse {
                branch,
                transaction_ack: None,
            });
        }

        None
    }

    /// Match an incoming request against an existing server transaction and,
    /// if it is a retransmission, return `Some(last_response)` to replay
    /// (`Some(None)` when no response has been sent yet). Returns `None` for
    /// a new request.
    ///
    /// Per RFC 3261 §17.2.3 a request matches a server transaction only if
    /// the top-Via branch AND the method match (ACK matches its INVITE and is
    /// routed separately before this check is consulted for it). Matching on
    /// branch alone silently swallowed CANCELs — a CANCEL reuses the
    /// cancelled INVITE's branch (§9.1) and was absorbed here as an "INVITE
    /// retransmission" with nothing to replay (issue #55).
    fn matched_retransmission(
        &self,
        request: &SipMessage,
        src: SocketAddr,
    ) -> Option<Option<SipMessage>> {
        let branch = Self::extract_branch(request)?;
        let method = request.method()?;
        if let Some(txn) = self.invite_server_txns.get(&branch) {
            if txn.request.method() == Some(method)
                && request_identity_matches(&txn.request, request)
                && txn.remote_addr == src
            {
                return Some(txn.last_response.clone());
            }
        }
        if let Some(txn) = self.non_invite_server_txns.get(&branch) {
            if txn.request.method() == Some(method)
                && request_identity_matches(&txn.request, request)
                && txn.remote_addr == src
            {
                return Some(txn.last_response.clone());
            }
        }
        if method == SipMethod::Invite {
            if let Some(cached) = self.invite_final_cache.get(&branch) {
                if cached.expires_at > Instant::now()
                    && cached.remote_addr == src
                    && request_identity_matches(&cached.request, request)
                {
                    return Some(Some(cached.response.clone()));
                }
            }
        }
        None
    }

    /// Cache a TU response in its non-INVITE server transaction. A final
    /// response enters Completed and arms Timer J; a duplicate request can
    /// then replay the exact response without reaching the TU again.
    fn record_non_invite_response(&mut self, response: &SipMessage, remote_addr: SocketAddr) {
        let Some(branch) = Self::extract_branch(response) else {
            return;
        };
        let Some(status) = response.status_code() else {
            return;
        };
        let Some(txn) = self.non_invite_server_txns.get_mut(&branch) else {
            return;
        };
        if txn.remote_addr != remote_addr
            || txn.request.call_id() != response.call_id()
            || cseq_number(&txn.request) != cseq_number(response)
            || txn.request.from_header().and_then(extract_tag)
                != response.from_header().and_then(extract_tag)
        {
            return;
        }

        if status < 200 {
            txn.send_provisional(response.clone());
        } else {
            txn.send_final(response.clone());
        }
    }

    /// A 2xx ACK has a new Via branch. Match it by the dialog identity,
    /// source tuple, and INVITE CSeq instead (RFC 3261 Section 13.2.2.4).
    fn accepted_ack_branch(&self, ack: &SipMessage, src: SocketAddr) -> Option<String> {
        let call_id = ack.call_id()?;
        let from_tag = ack.from_header().and_then(extract_tag)?;
        let to_tag = ack.to_header().and_then(extract_tag)?;
        let ack_cseq = cseq_number(ack)?;

        self.invite_server_txns.iter().find_map(|(branch, txn)| {
            if txn.state != crate::transaction::InviteServerState::Accepted
                || txn.remote_addr != src
                || txn.request.call_id() != Some(call_id)
                || txn.request.from_header().and_then(extract_tag).as_deref()
                    != Some(from_tag.as_str())
                || cseq_number(&txn.request) != Some(ack_cseq)
            {
                return None;
            }
            let response_to_tag = txn.last_response.as_ref()?
                .to_header().and_then(extract_tag)?;
            (response_to_tag == to_tag).then(|| branch.clone())
        })
    }

    /// Collect branches that need retransmission for INVITE client transactions.
    #[allow(dead_code)]
    fn retransmit_candidates(&self) -> Vec<String> {
        let mut result = Vec::new();
        for (branch, txn) in &self.invite_client_txns {
            if txn.needs_retransmit() {
                result.push(branch.clone());
            }
        }
        for (branch, txn) in &self.non_invite_client_txns {
            if txn.needs_retransmit() {
                result.push(branch.clone());
            }
        }
        result
    }

    /// Advance every terminal timer and return only abnormal timeouts that
    /// must be reported to the TU. D/I/J/K are normal absorption lifetimes;
    /// they terminate silently and are reaped in the same timer pass.
    fn expire_timers(&mut self) -> Vec<TransactionTimeoutEvent> {
        let mut events = Vec::new();
        for (branch, txn) in &mut self.invite_client_txns {
            if txn.timer_b_expired() {
                txn.terminate();
                events.push(TransactionTimeoutEvent {
                    branch: branch.clone(),
                    invite_ack_call_id: None,
                });
            } else if txn.timer_d_expired() {
                txn.terminate();
                debug!(branch = %branch, "Timer D expired");
            }
        }
        for (branch, txn) in &mut self.non_invite_client_txns {
            if txn.timer_f_expired() {
                txn.terminate();
                events.push(TransactionTimeoutEvent {
                    branch: branch.clone(),
                    invite_ack_call_id: None,
                });
            } else if txn.timer_k_expired() {
                txn.terminate();
                debug!(branch = %branch, "Timer K expired");
            }
        }
        for (branch, txn) in &mut self.invite_server_txns {
            if txn.timer_h_expired() {
                let invite_ack_call_id =
                    (txn.state == crate::transaction::InviteServerState::Accepted)
                        .then(|| txn.request.call_id().map(str::to_string))
                        .flatten();
                txn.terminate();
                events.push(TransactionTimeoutEvent {
                    branch: branch.clone(),
                    invite_ack_call_id,
                });
            } else if txn.timer_i_expired() {
                txn.terminate();
                debug!(branch = %branch, "Timer I expired");
            }
        }
        for (branch, txn) in &mut self.non_invite_server_txns {
            if txn.timer_j_expired() {
                txn.terminate();
                debug!(branch = %branch, "Timer J expired");
            }
        }
        events
    }
}

/// Dialog manager: tracks active dialogs by (call_id, local_tag, remote_tag).
#[allow(dead_code)]
struct DialogManager {
    dialogs: HashMap<String, Dialog>,
}

#[allow(dead_code)]
impl DialogManager {
    fn new() -> Self {
        Self {
            dialogs: HashMap::new(),
        }
    }

    /// Dialog key from a SIP message.
    fn dialog_key_from_msg(msg: &SipMessage, is_uas: bool) -> Option<String> {
        let call_id = msg.call_id()?;
        let from_hdr = msg.from_header()?;
        let to_hdr = msg.to_header()?;
        let from_tag = extract_tag(from_hdr).unwrap_or_default();
        let to_tag = extract_tag(to_hdr).unwrap_or_default();

        if is_uas {
            // UAS: local_tag is To, remote_tag is From
            Some(format!("{}:{}:{}", call_id, to_tag, from_tag))
        } else {
            // UAC: local_tag is From, remote_tag is To
            Some(format!("{}:{}:{}", call_id, from_tag, to_tag))
        }
    }

    fn insert(&mut self, dialog: Dialog) {
        let key = format!("{}:{}:{}", dialog.call_id, dialog.local_tag, dialog.remote_tag);
        self.dialogs.insert(key, dialog);
    }

    fn find_by_call_id(&self, call_id: &str) -> Option<&Dialog> {
        self.dialogs.values().find(|d| d.call_id == call_id)
    }

    fn remove_by_call_id(&mut self, call_id: &str) -> Option<Dialog> {
        let key = self
            .dialogs
            .iter()
            .find(|(_, d)| d.call_id == call_id)
            .map(|(k, _)| k.clone());
        key.and_then(|k| self.dialogs.remove(&k))
    }
}

/// Capacity of the stack → application event queue. Sized to absorb bursts;
/// when it fills, `emit_event` applies backpressure instead of dropping.
const EVENT_QUEUE_CAPACITY: usize = 256;

/// The SIP stack coordinator: wires transport, transactions, and dialogs.
pub struct SipStack {
    transport: Arc<UdpTransport>,
    transaction_layer: Arc<RwLock<TransactionLayer>>,
    #[allow(dead_code)]
    dialog_manager: Arc<RwLock<DialogManager>>,
    local_addr: SocketAddr,
    event_tx: mpsc::Sender<SipEvent>,
    event_rx: Option<mpsc::Receiver<SipEvent>>,
    /// Edge-trigger for the queue-full warning so a sustained burst logs
    /// once per episode instead of once per event.
    event_queue_full_logged: AtomicBool,
}

impl SipStack {
    /// Create a new SIP stack bound to the given address.
    pub async fn new(bind_addr: SocketAddr) -> Result<Self, TransportError> {
        let transport = UdpTransport::bind(bind_addr).await?;
        let local_addr = transport.local_addr()?;
        let (event_tx, event_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);

        info!(addr = %local_addr, "SIP stack created");

        Ok(Self {
            transport: Arc::new(transport),
            transaction_layer: Arc::new(RwLock::new(TransactionLayer::new())),
            dialog_manager: Arc::new(RwLock::new(DialogManager::new())),
            local_addr,
            event_tx,
            event_rx: Some(event_rx),
            event_queue_full_logged: AtomicBool::new(false),
        })
    }

    /// Get the local address the stack is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Take the event receiver. Can only be called once.
    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<SipEvent>> {
        self.event_rx.take()
    }

    /// Send a SIP message through the transport layer.
    pub async fn send_message(
        &self,
        msg: &SipMessage,
        addr: SocketAddr,
    ) -> Result<(), TransportError> {
        self.transport.send(msg, addr).await
    }

    /// Send an INVITE, creating a client transaction with timer management.
    pub async fn send_invite(
        &self,
        request: SipMessage,
        remote_addr: SocketAddr,
    ) -> Result<String, TransportError> {
        let branch = TransactionLayer::extract_branch(&request)
            .unwrap_or_else(|| format!("z9hG4bK{}", uuid::Uuid::new_v4()));

        // Send the initial request
        self.transport.send(&request, remote_addr).await?;

        // Create the client transaction
        let txn = ClientTransaction::new(request, remote_addr, branch.clone());
        self.transaction_layer
            .write()
            .invite_client_txns
            .insert(branch.clone(), txn);

        debug!(branch = %branch, "Created INVITE client transaction");
        Ok(branch)
    }

    /// Send a non-INVITE request, creating a client transaction.
    pub async fn send_request(
        &self,
        request: SipMessage,
        remote_addr: SocketAddr,
    ) -> Result<String, TransportError> {
        let branch = TransactionLayer::extract_branch(&request)
            .unwrap_or_else(|| format!("z9hG4bK{}", uuid::Uuid::new_v4()));

        self.transport.send(&request, remote_addr).await?;

        let txn = NonInviteClientTransaction::new(request, remote_addr, branch.clone());
        self.transaction_layer
            .write()
            .non_invite_client_txns
            .insert(branch.clone(), txn);

        debug!(branch = %branch, "Created non-INVITE client transaction");
        Ok(branch)
    }

    /// Send a response to an incoming request (UAS side).
    pub async fn send_response(
        &self,
        response: SipMessage,
        remote_addr: SocketAddr,
    ) -> Result<(), TransportError> {
        self.transaction_layer
            .write()
            .record_non_invite_response(&response, remote_addr);
        self.transport.send(&response, remote_addr).await
    }

    /// Snapshot the live transaction-map sizes for exact lifecycle checks.
    pub fn transaction_counts(&self) -> TransactionCounts {
        let layer = self.transaction_layer.read();
        TransactionCounts {
            invite_client: layer.invite_client_txns.len(),
            invite_server: layer.invite_server_txns.len(),
            non_invite_client: layer.non_invite_client_txns.len(),
            non_invite_server: layer.non_invite_server_txns.len(),
        }
    }

    /// Get a clone of the transport for use by external components
    /// (e.g., the event handler needs to send SIP responses).
    pub fn transport(&self) -> Arc<UdpTransport> {
        self.transport.clone()
    }

    /// Record a final response the application layer is about to send for an
    /// INVITE server transaction, and report whether sending it is allowed.
    ///
    /// Returns `false` when the transaction already holds a final response —
    /// e.g. a CANCEL-triggered 487 — in which case the caller must NOT put
    /// its response on the wire (RFC 3261 §9.2: a cancelled INVITE must not
    /// also be answered). The check-and-record is atomic under the
    /// transaction lock, which closes the CANCEL/Answer() race: whichever
    /// side records its final first wins, the other is suppressed.
    ///
    /// Recording also arms the transaction layer's retransmission machinery:
    /// non-2xx finals are re-sent by Timer G until the ACK arrives, and
    /// request retransmissions replay the recorded response.
    ///
    /// A request with no matching transaction (e.g. handler-level tests that
    /// bypass the stack) is allowed without recording.
    pub fn record_invite_final(&self, request: &SipMessage, response: &SipMessage) -> bool {
        let Some(branch) = TransactionLayer::extract_branch(request) else {
            return true;
        };
        let mut txn_layer = self.transaction_layer.write();
        match txn_layer.invite_server_txns.get_mut(&branch) {
            Some(txn) => {
                if txn.state == crate::transaction::InviteServerState::Proceeding {
                    let cached_request = txn.request.clone();
                    let remote_addr = txn.remote_addr;
                    txn.send_final(response.clone());
                    txn_layer.invite_final_cache.insert(branch, CachedInviteFinal {
                        request: cached_request,
                        remote_addr,
                        response: response.clone(),
                        expires_at: Instant::now() + crate::transaction::timers::TIMER_H,
                    });
                    true
                } else {
                    debug!(
                        branch = %branch,
                        "suppressing INVITE final: transaction already completed"
                    );
                    false
                }
            }
            None => true,
        }
    }

    /// Deliver a stack event to the application layer, never dropping it.
    ///
    /// `try_send` covers the common (non-full) case without a context
    /// switch. When the queue is full we log once per episode and fall back
    /// to an awaited `send`, which blocks the stack's event loop: incoming
    /// datagrams then queue in the kernel UDP buffer, where loss is
    /// recovered by SIP retransmission (RFC 3261 §17.1). Dropping here
    /// instead would lose signaling permanently — the request is already
    /// absorbed into a server transaction, so peer retransmissions are
    /// matched as duplicates and never re-emitted (issue #26).
    async fn emit_event(&self, event: SipEvent) {
        match self.event_tx.try_send(event) {
            Ok(()) => {
                self.event_queue_full_logged.store(false, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Full(event)) => {
                if !self.event_queue_full_logged.swap(true, Ordering::Relaxed) {
                    warn!(
                        capacity = EVENT_QUEUE_CAPACITY,
                        "SIP event queue full; applying backpressure to the receive loop"
                    );
                }
                if self.event_tx.send(event).await.is_err() {
                    error!("SIP event receiver dropped; event discarded");
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                error!("SIP event receiver dropped; event discarded");
            }
        }
    }

    /// Main event loop: recv from transport, route through transaction and
    /// dialog layers, emit events for the application.
    pub async fn run(&self) {
        let timer_interval = Duration::from_millis(50);

        loop {
            tokio::select! {
                // Receive from transport
                result = self.transport.recv() => {
                    match result {
                        Ok((msg, src)) => {
                            self.handle_incoming(msg, src).await;
                        }
                        Err(e) => {
                            warn!(error = %e, "Transport recv error");
                        }
                    }
                }

                // Timer tick for retransmissions and timeouts
                _ = tokio::time::sleep(timer_interval) => {
                    self.handle_timers().await;
                }
            }
        }
    }

    /// Handle an incoming SIP message.
    async fn handle_incoming(&self, msg: SipMessage, src: SocketAddr) {
        if msg.is_response() {
            self.handle_response(msg, src).await;
        } else {
            self.handle_request(msg, src).await;
        }
    }

    /// Handle an incoming response.
    async fn handle_response(&self, response: SipMessage, src: SocketAddr) {
        // Route through transaction layer
        let matched = {
            let mut txn_layer = self.transaction_layer.write();
            txn_layer.process_response(&response, src)
        };

        if let Some(ref matched) = matched {
            debug!(src = %src, "Response matched transaction");
            if let Some((ack, remote_addr)) = &matched.transaction_ack {
                if let Err(error) = self.transport.send(ack, *remote_addr).await {
                    warn!(branch = %matched.branch, %error, "Failed to send non-2xx ACK");
                } else {
                    debug!(branch = %matched.branch, "Sent non-2xx transaction ACK");
                }
            }
        } else {
            debug!(src = %src, "Response did not match any transaction (stray)");
        }

        // Emit event for application layer
        self.emit_event(SipEvent::Response {
            response,
            remote_addr: src,
        })
        .await;
    }

    /// Handle an incoming request.
    async fn handle_request(&self, mut request: SipMessage, src: SocketAddr) {
        let method = match request.method() {
            Some(m) => m,
            None => return,
        };

        // Stamp the top Via with received/rport for the packet source before
        // the request is cloned into transactions, sessions, and events. The
        // responses we build echo this Via verbatim, so a NAT'd client or a
        // downstream proxy gets the return-routing parameters RFC 3261
        // §18.2.1 / RFC 3581 require. Branch and other params are preserved,
        // so transaction matching is unaffected (issue #27).
        request.stamp_via_received_rport(src);

        // Check for retransmission (existing server transaction with the same
        // branch AND method, RFC 3261 §17.2.3). Extract the response to
        // replay (if any) without holding the lock across await.
        let retransmit_response = {
            let txn_layer = self.transaction_layer.read();
            txn_layer.matched_retransmission(&request, src)
        };

        if let Some(maybe_resp) = retransmit_response {
            debug!(method = ?method, "Retransmission of existing request");
            if let Some(resp) = maybe_resp {
                let _ = self.transport.send(&resp, src).await;
            }
            return;
        }

        match method {
            SipMethod::Invite => {
                // Create server transaction
                let branch = TransactionLayer::extract_branch(&request)
                    .unwrap_or_else(|| format!("z9hG4bK{}", uuid::Uuid::new_v4()));
                let txn = ServerTransaction::new(request.clone(), src, branch.clone());
                self.transaction_layer
                    .write()
                    .invite_server_txns
                    .insert(branch, txn);

                // Create inbound session
                if let Some(session) = SipSession::new_inbound(&request, self.local_addr, src) {
                    self.emit_event(SipEvent::IncomingInvite {
                        session,
                        request,
                        remote_addr: src,
                    })
                    .await;
                }
            }
            SipMethod::Ack => {
                // Non-2xx ACK is transaction-scoped and reuses the branch.
                // A 2xx ACK is a new-branch dialog request and reaches TU.
                let accepted_branch = self.transaction_layer.read()
                    .accepted_ack_branch(&request, src);
                if let Some(branch) = accepted_branch {
                    if let Some(txn) = self.transaction_layer.write()
                        .invite_server_txns.get_mut(&branch)
                    {
                        txn.on_ack();
                    }
                    let call_id = request.call_id().unwrap_or_default().to_string();
                    self.emit_event(SipEvent::IncomingAck {
                        call_id,
                        request,
                        remote_addr: src,
                    }).await;
                } else if let Some(branch) = TransactionLayer::extract_branch(&request) {
                    let mut txn_layer = self.transaction_layer.write();
                    if let Some(txn) = txn_layer.invite_server_txns.get_mut(&branch) {
                        if txn.remote_addr == src
                            && txn.request.call_id() == request.call_id()
                            && cseq_number(&txn.request) == cseq_number(&request)
                        {
                            txn.on_ack();
                            debug!(branch = %branch, "ACK received for INVITE server transaction");
                        }
                    }
                }
            }
            SipMethod::Cancel => {
                self.handle_cancel_request(request, src).await;
            }
            SipMethod::Bye => {
                // Create non-INVITE server transaction
                let branch = TransactionLayer::extract_branch(&request)
                    .unwrap_or_else(|| format!("z9hG4bK{}", uuid::Uuid::new_v4()));
                let txn = NonInviteServerTransaction::new(request.clone(), src, branch.clone());
                self.transaction_layer
                    .write()
                    .non_invite_server_txns
                    .insert(branch, txn);

                let call_id = request.call_id().unwrap_or("").to_string();
                self.emit_event(SipEvent::IncomingBye {
                    call_id,
                    request,
                    remote_addr: src,
                })
                .await;
            }
            _ => {
                // Create non-INVITE server transaction
                let branch = TransactionLayer::extract_branch(&request)
                    .unwrap_or_else(|| format!("z9hG4bK{}", uuid::Uuid::new_v4()));
                let txn = NonInviteServerTransaction::new(request.clone(), src, branch.clone());
                self.transaction_layer
                    .write()
                    .non_invite_server_txns
                    .insert(branch, txn);

                self.emit_event(SipEvent::IncomingRequest {
                    request,
                    remote_addr: src,
                })
                .await;
            }
        }
    }

    /// Handle an incoming CANCEL (RFC 3261 §9.2).
    ///
    /// A CANCEL forms its own (non-INVITE) server transaction but is matched
    /// to the INVITE it cancels by the shared top-Via branch (§9.1). The
    /// transaction layer answers both sides itself — `200 OK` to the CANCEL
    /// and `487 Request Terminated` to the still-pending INVITE — recording
    /// each in its transaction so retransmissions replay correctly. The
    /// application layer is then told via `IncomingCancel` to abort the
    /// channel and its dialplan. A CANCEL that arrives after the INVITE got
    /// its final response still gets `200 OK` but has no further effect; a
    /// CANCEL matching no transaction gets `481` (§9.2).
    async fn handle_cancel_request(&self, request: SipMessage, src: SocketAddr) {
        let branch = TransactionLayer::extract_branch(&request)
            .unwrap_or_else(|| format!("z9hG4bK{}", uuid::Uuid::new_v4()));

        // A matching CANCEL copies the INVITE's identity (RFC 3261 §9.1):
        // beyond the shared branch it must carry the same Call-ID, From tag,
        // and CSeq number. Guard against a colliding/forged branch
        // terminating an unrelated INVITE.
        let identity_matches = |invite: &SipMessage, cancel: &SipMessage| {
            invite.call_id() == cancel.call_id()
                && cseq_number(invite) == cseq_number(cancel)
                && invite.from_header().and_then(extract_tag)
                    == cancel.from_header().and_then(extract_tag)
        };

        // Resolve everything under one lock; send after dropping it.
        let (cancel_response, invite_response, cancelled_call_id) = {
            let mut txn_layer = self.transaction_layer.write();

            let mut invite_response = None;
            let mut cancelled_call_id = None;

            let matched_txn = txn_layer
                .invite_server_txns
                .get_mut(&branch)
                .filter(|txn| identity_matches(&txn.request, &request));

            let cancel_response = match matched_txn {
                Some(invite_txn) => {
                    if invite_txn.state == crate::transaction::InviteServerState::Proceeding {
                        // INVITE still pending: terminate it with 487.
                        if let Ok(resp) =
                            invite_txn.request.create_response(487, "Request Terminated")
                        {
                            invite_txn.send_final(resp.clone());
                            invite_response = Some(resp);
                        }
                        cancelled_call_id =
                            request.call_id().map(|s| s.to_string());
                    } else {
                        // Final response already sent; the CANCEL still gets
                        // its 200 OK but has no effect (§9.2).
                        debug!(branch = %branch, "CANCEL after final response; no effect");
                    }
                    request.create_response(200, "OK").ok()
                }
                None => {
                    debug!(branch = %branch, "CANCEL matched no INVITE transaction");
                    request
                        .create_response(481, "Call/Transaction Does Not Exist")
                        .ok()
                }
            };

            // Absorb CANCEL retransmissions: give the CANCEL its own server
            // transaction with the response recorded for replay.
            let mut cancel_txn =
                NonInviteServerTransaction::new(request.clone(), src, branch.clone());
            if let Some(ref resp) = cancel_response {
                cancel_txn.send_final(resp.clone());
            }
            txn_layer
                .non_invite_server_txns
                .insert(branch.clone(), cancel_txn);

            (cancel_response, invite_response, cancelled_call_id)
        };

        if let Some(resp) = cancel_response {
            let _ = self.transport.send(&resp, src).await;
        }
        if let Some(resp) = invite_response {
            info!(branch = %branch, "CANCEL terminated pending INVITE; sent 487");
            let _ = self.transport.send(&resp, src).await;
        }
        if let Some(call_id) = cancelled_call_id {
            self.emit_event(SipEvent::IncomingCancel {
                call_id,
                request,
                remote_addr: src,
            })
            .await;
        }
    }

    /// Handle timer-driven retransmissions and timeouts.
    async fn handle_timers(&self) {
        // Collect retransmit candidates
        let retransmit_branches: Vec<(SipMessage, SocketAddr, String, bool)> = {
            let txn_layer = self.transaction_layer.read();
            let mut candidates = Vec::new();
            for (branch, txn) in &txn_layer.invite_client_txns {
                if txn.needs_retransmit() {
                    candidates.push((
                        txn.request.clone(),
                        txn.remote_addr,
                        branch.clone(),
                        true, // is_invite
                    ));
                }
            }
            for (branch, txn) in &txn_layer.non_invite_client_txns {
                if txn.needs_retransmit() {
                    candidates.push((
                        txn.request.clone(),
                        txn.remote_addr,
                        branch.clone(),
                        false,
                    ));
                }
            }
            candidates
        };

        // Perform retransmissions
        for (request, addr, branch, is_invite) in retransmit_branches {
            debug!(branch = %branch, "Retransmitting request");
            if let Err(e) = self.transport.send(&request, addr).await {
                error!(branch = %branch, error = %e, "Retransmit failed");
            }
            let mut txn_layer = self.transaction_layer.write();
            if is_invite {
                if let Some(txn) = txn_layer.invite_client_txns.get_mut(&branch) {
                    txn.advance_retransmit_timer();
                }
            } else {
                if let Some(txn) = txn_layer.non_invite_client_txns.get_mut(&branch) {
                    txn.advance_retransmit_timer();
                }
            }
        }

        // Timer G: retransmit final non-2xx responses on INVITE server
        // transactions until the ACK arrives (RFC 3261 §17.2.1). Without
        // this, a lost 487/4xx datagram hangs the caller's transaction —
        // after 100 Trying the UAC stops retransmitting the INVITE, so the
        // request-retransmission replay path never fires (issue #55 review).
        let server_retransmits: Vec<(SipMessage, SocketAddr, String)> = {
            let txn_layer = self.transaction_layer.read();
            txn_layer
                .invite_server_txns
                .iter()
                .filter(|(_, txn)| txn.needs_retransmit())
                .filter_map(|(branch, txn)| {
                    txn.last_response
                        .clone()
                        .map(|resp| (resp, txn.remote_addr, branch.clone()))
                })
                .collect()
        };

        for (response, addr, branch) in server_retransmits {
            debug!(branch = %branch, "Timer G: retransmitting final response");
            if let Err(e) = self.transport.send(&response, addr).await {
                error!(branch = %branch, error = %e, "Response retransmit failed");
            }
            let mut txn_layer = self.transaction_layer.write();
            if let Some(txn) = txn_layer.invite_server_txns.get_mut(&branch) {
                txn.advance_retransmit_timer();
            }
        }

        // Advance B/D/F/H/I/J/K. Only B/F/H are abnormal timeouts delivered
        // to the TU; D/I/J/K simply end their retransmission-absorption wait.
        let timed_out = self.transaction_layer.write().expire_timers();

        for timeout in timed_out {
            warn!(branch = %timeout.branch, "Transaction timed out");
            self.emit_event(SipEvent::TransactionTimeout {
                branch: timeout.branch,
            })
            .await;
            if let Some(call_id) = timeout.invite_ack_call_id {
                self.emit_event(SipEvent::InviteAckTimeout { call_id }).await;
            }
        }

        // Clean up terminated transactions
        let mut txn_layer = self.transaction_layer.write();
        txn_layer
            .invite_client_txns
            .retain(|_, t| t.state != crate::transaction::InviteClientState::Terminated);
        txn_layer.invite_server_txns.retain(|_, t| {
            t.state != crate::transaction::InviteServerState::Terminated
        });
        txn_layer.non_invite_client_txns.retain(|_, t| {
            t.state != crate::transaction::NonInviteClientState::Terminated
        });
        txn_layer.non_invite_server_txns.retain(|_, t| {
            t.state != crate::transaction::NonInviteServerState::Terminated
        });
        let now = Instant::now();
        txn_layer.invite_final_cache.retain(|_, cached| cached.expires_at > now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sip_stack_create_and_send() {
        // Bind to any available port
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let local_addr = stack.local_addr();
        assert_ne!(local_addr.port(), 0);

        // We should be able to take the event receiver
        let rx = stack.take_event_rx();
        assert!(rx.is_some());

        // Second take should return None
        let rx2 = stack.take_event_rx();
        assert!(rx2.is_none());
    }

    #[tokio::test]
    async fn test_sip_stack_send_recv_message() {
        use crate::parser::{SipHeader, SipUri, RequestLine, StartLine, SipMethod};

        // Create two stacks
        let addr1: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let addr2: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let stack1 = SipStack::new(addr1).await.unwrap();
        let mut stack2 = SipStack::new(addr2).await.unwrap();

        let stack2_addr = stack2.local_addr();
        let stack1_addr = stack1.local_addr();
        let mut rx2 = stack2.take_event_rx().unwrap();

        // Spawn stack2's event loop
        let stack2_arc = Arc::new(stack2);
        let stack2_run = stack2_arc.clone();
        let handle = tokio::spawn(async move {
            // Run for a short time
            tokio::select! {
                _ = stack2_run.run() => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        });

        // Build an OPTIONS request from stack1 to stack2
        let uri = SipUri::parse(&format!("sip:{}", stack2_addr)).unwrap();
        let branch = format!("z9hG4bKtest{}", uuid::Uuid::new_v4().to_string().replace('-', ""));
        let call_id = format!("test-{}", uuid::Uuid::new_v4());
        let msg = SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Options,
                uri,
                version: "SIP/2.0".to_string(),
            }),
            headers: vec![
                SipHeader {
                    name: header_names::VIA.to_string(),
                    value: format!("SIP/2.0/UDP {};branch={}", stack1_addr, branch),
                },
                SipHeader {
                    name: header_names::FROM.to_string(),
                    value: format!("<sip:test@{}>;tag=test123", stack1_addr),
                },
                SipHeader {
                    name: header_names::TO.to_string(),
                    value: format!("<sip:test@{}>", stack2_addr),
                },
                SipHeader {
                    name: header_names::CALL_ID.to_string(),
                    value: call_id.clone(),
                },
                SipHeader {
                    name: header_names::CSEQ.to_string(),
                    value: "1 OPTIONS".to_string(),
                },
                SipHeader {
                    name: header_names::CONTENT_LENGTH.to_string(),
                    value: "0".to_string(),
                },
            ],
            body: String::new(),
        };

        // Send via transport directly
        stack1.send_message(&msg, stack2_addr).await.unwrap();

        // Wait for the event on stack2
        let event = tokio::time::timeout(Duration::from_secs(2), rx2.recv())
            .await
            .expect("timeout waiting for event")
            .expect("channel closed");

        match event {
            SipEvent::IncomingRequest { request, remote_addr: _ } => {
                assert_eq!(request.method(), Some(SipMethod::Options));
                assert_eq!(request.call_id(), Some(call_id.as_str()));
            }
            other => panic!("Expected IncomingRequest, got {:?}", other),
        }

        handle.abort();
    }

    /// Build a minimal OPTIONS request for feeding handle_request directly.
    fn build_options_request(call_id: &str, branch: &str) -> SipMessage {
        use crate::parser::{SipHeader, SipUri, RequestLine, StartLine, SipMethod};

        SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Options,
                uri: SipUri::parse("sip:127.0.0.1:5060").unwrap(),
                version: "SIP/2.0".to_string(),
            }),
            headers: vec![
                SipHeader {
                    name: header_names::VIA.to_string(),
                    value: format!("SIP/2.0/UDP 127.0.0.1:5061;branch={}", branch),
                },
                SipHeader {
                    name: header_names::FROM.to_string(),
                    value: "<sip:test@127.0.0.1:5061>;tag=t1".to_string(),
                },
                SipHeader {
                    name: header_names::TO.to_string(),
                    value: "<sip:test@127.0.0.1:5060>".to_string(),
                },
                SipHeader {
                    name: header_names::CALL_ID.to_string(),
                    value: call_id.to_string(),
                },
                SipHeader {
                    name: header_names::CSEQ.to_string(),
                    value: "1 OPTIONS".to_string(),
                },
                SipHeader {
                    name: header_names::CONTENT_LENGTH.to_string(),
                    value: "0".to_string(),
                },
            ],
            body: String::new(),
        }
    }

    /// Regression test for issue #26: events emitted while the queue is full
    /// must be delivered once the consumer drains (backpressure), never
    /// silently dropped. Before the fix, everything past EVENT_QUEUE_CAPACITY
    /// was lost and this test timed out waiting for the missing events.
    #[tokio::test]
    async fn test_emit_event_backpressure_never_drops() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();
        let stack = Arc::new(stack);

        const TOTAL: usize = EVENT_QUEUE_CAPACITY + 44;

        let producer = {
            let stack = stack.clone();
            tokio::spawn(async move {
                for i in 0..TOTAL {
                    stack
                        .emit_event(SipEvent::TransactionTimeout {
                            branch: format!("b{}", i),
                        })
                        .await;
                }
            })
        };

        // Let the producer run into the full queue so the overflow path
        // (block, don't drop) is actually exercised before we drain.
        tokio::time::sleep(Duration::from_millis(100)).await;

        for expected in 0..TOTAL {
            let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "event {} never delivered — events were dropped under load",
                        expected
                    )
                })
                .expect("channel closed");
            match event {
                SipEvent::TransactionTimeout { branch } => {
                    assert_eq!(branch, format!("b{}", expected), "events reordered");
                }
                other => panic!("Expected TransactionTimeout, got {:?}", other),
            }
        }

        producer.await.unwrap();
    }

    /// Regression test for issue #26 at the request-handling call site: an
    /// incoming request that arrives while the event queue is full must
    /// still reach the application. Before the fix, handle_request absorbed
    /// the request into a server transaction and then dropped the event —
    /// retransmissions would match the transaction and be blackholed.
    #[tokio::test]
    async fn test_handle_request_delivers_event_when_queue_full() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();
        let stack = Arc::new(stack);

        // Fill the queue to exactly its capacity (none of these block).
        for i in 0..EVENT_QUEUE_CAPACITY {
            stack
                .emit_event(SipEvent::TransactionTimeout {
                    branch: format!("filler{}", i),
                })
                .await;
        }

        // Deliver a request while the queue is full; it must park in
        // backpressure rather than dropping the IncomingRequest event.
        let src: SocketAddr = "127.0.0.1:5061".parse().unwrap();
        let request = build_options_request("queue-full-call", "z9hG4bKqueuefull");
        let handler = {
            let stack = stack.clone();
            tokio::spawn(async move {
                stack.handle_request(request, src).await;
            })
        };

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Drain the fillers, then the request event must arrive.
        for _ in 0..EVENT_QUEUE_CAPACITY {
            let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("timeout draining filler events")
                .expect("channel closed");
            assert!(matches!(event, SipEvent::TransactionTimeout { .. }));
        }

        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("IncomingRequest never delivered — dropped while queue was full")
            .expect("channel closed");
        match event {
            SipEvent::IncomingRequest { request, .. } => {
                assert_eq!(request.call_id(), Some("queue-full-call"));
            }
            other => panic!("Expected IncomingRequest, got {:?}", other),
        }

        handler.await.unwrap();
    }

    /// Build a minimal INVITE for feeding handle_request directly.
    fn build_invite_request(call_id: &str, branch: &str) -> SipMessage {
        let raw = format!(
            "INVITE sip:100@127.0.0.1 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 127.0.0.1;branch={branch}\r\n\
             From: <sip:caller@127.0.0.1>;tag=c55\r\n\
             To: <sip:100@127.0.0.1>\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: 1 INVITE\r\n\
             Contact: <sip:caller@127.0.0.1:5062>\r\n\
             Content-Length: 0\r\n\r\n"
        );
        SipMessage::parse(raw.as_bytes()).unwrap()
    }

    /// Build the CANCEL for `build_invite_request` (same branch, RFC 3261 §9.1).
    fn build_cancel_request(call_id: &str, branch: &str) -> SipMessage {
        let raw = format!(
            "CANCEL sip:100@127.0.0.1 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 127.0.0.1;branch={branch}\r\n\
             From: <sip:caller@127.0.0.1>;tag=c55\r\n\
             To: <sip:100@127.0.0.1>\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: 1 CANCEL\r\n\
             Content-Length: 0\r\n\r\n"
        );
        SipMessage::parse(raw.as_bytes()).unwrap()
    }

    /// Receive one SIP datagram on the test peer socket, bounded by 2s.
    async fn recv_peer(sock: &tokio::net::UdpSocket) -> SipMessage {
        let mut buf = [0u8; 4096];
        let (len, _src) = tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf))
            .await
            .expect("timed out waiting for a SIP response")
            .expect("recv failed");
        SipMessage::parse(&buf[..len]).expect("response must parse")
    }

    /// Regression test for issue #55: a CANCEL matching a pending INVITE must
    /// get 200 OK, the INVITE must get 487 Request Terminated, and the
    /// application layer must see an IncomingCancel event (RFC 3261 §9.2).
    /// Before the fix, branch-only transaction matching swallowed the CANCEL
    /// as an "INVITE retransmission" and nothing happened at all.
    #[tokio::test]
    async fn test_cancel_terminates_pending_invite() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();

        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();

        let branch = "z9hG4bKcxl55";
        stack
            .handle_request(build_invite_request("cancel-55-1", branch), peer_addr)
            .await;
        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout")
            .expect("channel closed");
        assert!(matches!(event, SipEvent::IncomingInvite { .. }));

        stack
            .handle_request(build_cancel_request("cancel-55-1", branch), peer_addr)
            .await;

        // Both responses arrive on the wire (order not significant): 200 for
        // the CANCEL transaction, 487 for the INVITE transaction.
        let mut got = Vec::new();
        for _ in 0..2 {
            let resp = recv_peer(&peer).await;
            got.push((
                resp.status_code().unwrap_or(0),
                resp.get_header(header_names::CSEQ).unwrap_or("").to_string(),
            ));
        }
        assert!(
            got.contains(&(200, "1 CANCEL".to_string())),
            "CANCEL must be answered 200 OK, got {:?}",
            got
        );
        assert!(
            got.contains(&(487, "1 INVITE".to_string())),
            "pending INVITE must be terminated with 487, got {:?}",
            got
        );

        // The application layer is told to abort the channel.
        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("IncomingCancel event never emitted")
            .expect("channel closed");
        match event {
            SipEvent::IncomingCancel { call_id, .. } => {
                assert_eq!(call_id, "cancel-55-1");
            }
            other => panic!("Expected IncomingCancel, got {:?}", other),
        }

        // Retransmitted CANCEL replays the CANCEL's 200 (not the INVITE's
        // 487): server-transaction matching is branch AND method (§17.2.3).
        stack
            .handle_request(build_cancel_request("cancel-55-1", branch), peer_addr)
            .await;
        let resp = recv_peer(&peer).await;
        assert_eq!(resp.status_code(), Some(200));
        assert_eq!(resp.get_header(header_names::CSEQ), Some("1 CANCEL"));

        // Retransmitted INVITE replays the recorded 487 and must NOT re-emit
        // IncomingInvite (it is a retransmission, not a new call).
        stack
            .handle_request(build_invite_request("cancel-55-1", branch), peer_addr)
            .await;
        let resp = recv_peer(&peer).await;
        assert_eq!(resp.status_code(), Some(487));
        assert_eq!(resp.get_header(header_names::CSEQ), Some("1 INVITE"));
        assert!(
            rx.try_recv().is_err(),
            "retransmissions must not emit further events"
        );
    }

    /// Receive one SIP datagram if one arrives within `ms`, else None.
    async fn recv_peer_opt(sock: &tokio::net::UdpSocket, ms: u64) -> Option<SipMessage> {
        let mut buf = [0u8; 4096];
        let (len, _src) =
            tokio::time::timeout(Duration::from_millis(ms), sock.recv_from(&mut buf))
                .await
                .ok()?
                .ok()?;
        SipMessage::parse(&buf[..len]).ok()
    }

    /// A CANCEL that arrives after the INVITE already got its final response
    /// gets 200 OK but has NO effect: no 487, no IncomingCancel (RFC 3261
    /// §9.2). The application layer reports its finals through
    /// record_invite_final, so the transaction layer knows the INVITE was
    /// answered — before this wiring, a late CANCEL tore down established
    /// calls with a bogus 487.
    #[tokio::test]
    async fn test_cancel_after_final_response_has_no_effect() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();

        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();

        let branch = "z9hG4bKlate55";
        let invite = build_invite_request("cancel-55-late", branch);
        stack.handle_request(invite.clone(), peer_addr).await;
        let _ = rx.recv().await; // IncomingInvite

        // The application answers (as the Answer() path does).
        let ok = invite.create_response(200, "OK").unwrap();
        assert!(
            stack.record_invite_final(&invite, &ok),
            "first final response must be allowed"
        );

        // Late CANCEL: answered 200, but the call must be left alone.
        stack
            .handle_request(build_cancel_request("cancel-55-late", branch), peer_addr)
            .await;
        let resp = recv_peer(&peer).await;
        assert_eq!(resp.status_code(), Some(200));
        assert_eq!(resp.get_header(header_names::CSEQ), Some("1 CANCEL"));
        assert!(
            recv_peer_opt(&peer, 300).await.is_none(),
            "no 487 may follow a CANCEL on an answered INVITE"
        );
        assert!(
            rx.try_recv().is_err(),
            "a no-effect CANCEL must not reach the application layer"
        );
    }

    /// The CANCEL/Answer() race is resolved atomically in the transaction
    /// layer: once a CANCEL recorded its 487, record_invite_final refuses
    /// the answer (returns false), so the 200 OK never hits the wire.
    #[tokio::test]
    async fn test_record_invite_final_suppresses_answer_after_cancel() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();

        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();

        let branch = "z9hG4bKrace55";
        let invite = build_invite_request("cancel-55-race", branch);
        stack.handle_request(invite.clone(), peer_addr).await;
        let _ = rx.recv().await; // IncomingInvite

        stack
            .handle_request(build_cancel_request("cancel-55-race", branch), peer_addr)
            .await;

        // The CANCEL won: the racing answer must be suppressed.
        let ok = invite.create_response(200, "OK").unwrap();
        assert!(
            !stack.record_invite_final(&invite, &ok),
            "an answer racing a CANCEL-sent 487 must be suppressed"
        );
    }

    /// A CANCEL whose branch matches but whose identity (Call-ID here) does
    /// not belong to the INVITE must not terminate it (RFC 3261 §9.1: a
    /// matching CANCEL copies the INVITE's Call-ID, From tag, and CSeq
    /// number). It is answered 481 as an unmatched CANCEL.
    #[tokio::test]
    async fn test_cancel_identity_mismatch_gets_481() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();

        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();

        let branch = "z9hG4bKident55";
        stack
            .handle_request(build_invite_request("cancel-55-real", branch), peer_addr)
            .await;
        let _ = rx.recv().await; // IncomingInvite

        // Same branch, different Call-ID: must NOT cancel the INVITE.
        stack
            .handle_request(build_cancel_request("cancel-55-forged", branch), peer_addr)
            .await;
        let resp = recv_peer(&peer).await;
        assert_eq!(
            resp.status_code(),
            Some(481),
            "an identity-mismatched CANCEL must be rejected with 481"
        );
        assert!(
            recv_peer_opt(&peer, 300).await.is_none(),
            "the unrelated INVITE must not receive a 487"
        );
        assert!(rx.try_recv().is_err(), "no IncomingCancel for a forged CANCEL");
    }

    /// Timer G: the CANCEL-triggered 487 is retransmitted until the ACK
    /// arrives (RFC 3261 §17.2.1). Without this, a lost 487 hangs the
    /// caller's INVITE transaction — after 100 Trying the UAC no longer
    /// retransmits the INVITE, so replay-on-retransmission never fires.
    #[tokio::test]
    async fn test_timer_g_retransmits_487_until_ack() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();

        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();

        let branch = "z9hG4bKtimerg55";
        stack
            .handle_request(build_invite_request("cancel-55-tg", branch), peer_addr)
            .await;
        let _ = rx.recv().await; // IncomingInvite
        stack
            .handle_request(build_cancel_request("cancel-55-tg", branch), peer_addr)
            .await;
        let _ = rx.recv().await; // IncomingCancel
        // Drain the immediate 200 + 487.
        let _ = recv_peer(&peer).await;
        let _ = recv_peer(&peer).await;

        // After T1 (500ms) with no ACK, the timer pass must re-send the 487.
        tokio::time::sleep(Duration::from_millis(600)).await;
        stack.handle_timers().await;
        let retrans = recv_peer(&peer).await;
        assert_eq!(
            retrans.status_code(),
            Some(487),
            "Timer G must retransmit the un-ACKed 487"
        );
        assert_eq!(retrans.get_header(header_names::CSEQ), Some("1 INVITE"));

        // The ACK confirms the transaction; retransmission stops.
        let ack_raw = format!(
            "ACK sip:100@127.0.0.1 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 127.0.0.1;branch={branch}\r\n\
             From: <sip:caller@127.0.0.1>;tag=c55\r\n\
             To: <sip:100@127.0.0.1>\r\n\
             Call-ID: cancel-55-tg\r\n\
             CSeq: 1 ACK\r\n\
             Content-Length: 0\r\n\r\n"
        );
        stack
            .handle_request(SipMessage::parse(ack_raw.as_bytes()).unwrap(), peer_addr)
            .await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        stack.handle_timers().await;
        assert!(
            recv_peer_opt(&peer, 300).await.is_none(),
            "no further retransmissions after the ACK confirms the transaction"
        );
    }

    fn build_2xx_ack(call_id: &str, branch: &str, to_tag: &str) -> SipMessage {
        let raw = format!(
            "ACK sip:100@127.0.0.1 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 127.0.0.1;branch={branch}\r\n\
             From: <sip:caller@127.0.0.1>;tag=c55\r\n\
             To: <sip:100@127.0.0.1>;tag={to_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: 1 ACK\r\n\
             Content-Length: 0\r\n\r\n"
        );
        SipMessage::parse(raw.as_bytes()).unwrap()
    }

    /// A TU final completes a non-INVITE server transaction, duplicates get
    /// the byte-identical cached response without another event, and Timer J
    /// removes the map entry.
    #[tokio::test]
    async fn test_options_final_replays_and_timer_j_reaps_transaction() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();
        let branch = "z9hG4bKoptions-complete";
        let request = build_options_request("options-complete", branch);

        stack.handle_request(request.clone(), peer_addr).await;
        assert!(matches!(rx.recv().await, Some(SipEvent::IncomingRequest { .. })));
        assert_eq!(stack.transaction_counts().non_invite_server, 1);

        let response = request.create_response(200, "OK").unwrap();
        stack.send_response(response.clone(), peer_addr).await.unwrap();
        assert_eq!(recv_peer(&peer).await.to_string(), response.to_string());
        assert_eq!(
            stack.transaction_layer.read().non_invite_server_txns
                .get(branch).unwrap().state,
            crate::transaction::NonInviteServerState::Completed,
        );

        stack.handle_request(request, peer_addr).await;
        assert_eq!(recv_peer(&peer).await.to_string(), response.to_string());
        assert!(rx.try_recv().is_err(), "duplicate OPTIONS must not reach the TU");

        stack.transaction_layer.write().non_invite_server_txns
            .get_mut(branch).unwrap().expire_timer_j();
        stack.handle_timers().await;
        assert_eq!(stack.transaction_counts().non_invite_server, 0);
    }

    /// The TU retains and retransmits a successful final until its dialog ACK
    /// arrives. The ACK uses a different branch and reaches the application.
    #[tokio::test]
    async fn test_2xx_retransmits_until_new_branch_ack_reaches_tu() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();

        let invite = build_invite_request("accepted-call", "z9hG4bKaccepted");
        stack.handle_request(invite.clone(), peer_addr).await;
        let _ = rx.recv().await;
        let mut ok = invite.create_response(200, "OK").unwrap();
        ok.headers.iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case(header_names::TO))
            .unwrap().value.push_str(";tag=server-tag");
        assert!(stack.record_invite_final(&invite, &ok));
        stack.send_response(ok.clone(), peer_addr).await.unwrap();
        assert_eq!(recv_peer(&peer).await.to_string(), ok.to_string());

        tokio::time::sleep(Duration::from_millis(600)).await;
        stack.handle_timers().await;
        assert_eq!(recv_peer(&peer).await.to_string(), ok.to_string(),
            "the cached 2xx must be byte-equivalent");

        let ack = build_2xx_ack("accepted-call", "z9hG4bKnew-ack-branch", "server-tag");
        stack.handle_request(ack.clone(), peer_addr).await;
        match rx.recv().await.expect("2xx ACK must reach TU") {
            SipEvent::IncomingAck { call_id, request, remote_addr } => {
                assert_eq!(call_id, "accepted-call");
                assert_eq!(request.to_string(), ack.to_string());
                assert_eq!(remote_addr, peer_addr);
            }
            other => panic!("Expected IncomingAck, got {other:?}"),
        }

        tokio::time::sleep(Duration::from_millis(1100)).await;
        stack.handle_timers().await;
        assert!(recv_peer_opt(&peer, 300).await.is_none());
    }

    #[tokio::test]
    async fn test_2xx_ack_timeout_reaches_tu() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();
        let peer_addr: SocketAddr = "127.0.0.1:5061".parse().unwrap();
        let branch = "z9hG4bKack-timeout";
        let invite = build_invite_request("ack-timeout-call", branch);
        stack.handle_request(invite.clone(), peer_addr).await;
        let _ = rx.recv().await;
        let mut ok = invite.create_response(200, "OK").unwrap();
        ok.headers.iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case(header_names::TO))
            .unwrap().value.push_str(";tag=server-tag");
        assert!(stack.record_invite_final(&invite, &ok));
        stack.transaction_layer.write().invite_server_txns
            .get_mut(branch).unwrap().expire_final_response_timer();

        stack.handle_timers().await;
        assert!(matches!(rx.recv().await, Some(SipEvent::TransactionTimeout { .. })));
        assert!(matches!(rx.recv().await,
            Some(SipEvent::InviteAckTimeout { ref call_id }) if call_id == "ack-timeout-call"));
    }

    /// Even after the successful transaction is reaped on ACK, repeated
    /// copies of the original INVITE replay the cached final and never mint a
    /// second IncomingInvite/application call.
    #[tokio::test]
    async fn test_late_invite_retransmissions_replay_final_without_second_call() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();
        let branch = "z9hG4bKlate-final";
        let invite = build_invite_request("one-call-only", branch);

        stack.handle_request(invite.clone(), peer_addr).await;
        assert!(matches!(rx.recv().await, Some(SipEvent::IncomingInvite { .. })));
        let mut ok = invite.create_response(200, "OK").unwrap();
        ok.headers.iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case(header_names::TO))
            .unwrap().value.push_str(";tag=one-server-dialog");
        assert!(stack.record_invite_final(&invite, &ok));
        stack.send_response(ok.clone(), peer_addr).await.unwrap();

        let ack = build_2xx_ack(
            "one-call-only", "z9hG4bKone-call-ack", "one-server-dialog");
        stack.handle_request(ack, peer_addr).await;
        assert!(matches!(rx.recv().await, Some(SipEvent::IncomingAck { .. })));
        stack.handle_timers().await;
        assert!(!stack.transaction_layer.read().invite_server_txns.contains_key(branch));

        let mut observed = vec![recv_peer(&peer).await.to_string()];
        for _ in 0..2 {
            stack.handle_request(invite.clone(), peer_addr).await;
            observed.push(recv_peer(&peer).await.to_string());
        }
        assert_eq!(observed, vec![ok.to_string(); 3]);
        assert!(rx.try_recv().is_err(), "exactly one call may reach the TU");
    }

    #[test]
    fn test_full_b_d_f_h_i_j_k_timer_matrix() {
        use crate::transaction::{
            InviteClientState, InviteServerState, NonInviteClientState,
            NonInviteServerState,
        };

        let remote: SocketAddr = "127.0.0.1:5061".parse().unwrap();
        let invite = build_invite_request("timer-matrix", "z9hG4bKmatrix");
        let options = build_options_request("timer-matrix-options", "z9hG4bKoptions");

        let mut layer = TransactionLayer::new();

        // B remains armed after a provisional response moves Calling -> Proceeding.
        let mut b = ClientTransaction::new(invite.clone(), remote, "timer-b".to_string());
        b.on_response(invite.create_response(180, "Ringing").unwrap());
        assert_eq!(b.state, InviteClientState::Proceeding);
        b.expire_timer_b();
        layer.invite_client_txns.insert("timer-b".to_string(), b);

        // D retains the client transaction after it generated a non-2xx ACK.
        let mut d = ClientTransaction::new(invite.clone(), remote, "timer-d".to_string());
        d.on_response(invite.create_response(486, "Busy Here").unwrap());
        d.expire_timer_d();
        layer.invite_client_txns.insert("timer-d".to_string(), d);

        // F remains armed in Proceeding, just like B.
        let mut f = NonInviteClientTransaction::new(
            options.clone(), remote, "timer-f".to_string());
        f.on_response(options.create_response(100, "Trying").unwrap());
        assert_eq!(f.state, NonInviteClientState::Proceeding);
        f.expire_timer_f();
        layer.non_invite_client_txns.insert("timer-f".to_string(), f);

        // H ends a non-2xx server transaction that never receives ACK.
        let mut h = ServerTransaction::new(invite.clone(), remote, "timer-h".to_string());
        h.send_final(invite.create_response(486, "Busy Here").unwrap());
        h.expire_final_response_timer();
        layer.invite_server_txns.insert("timer-h".to_string(), h);

        // I absorbs ACK retransmissions from Confirmed before reaping.
        let mut i = ServerTransaction::new(invite.clone(), remote, "timer-i".to_string());
        i.send_final(invite.create_response(486, "Busy Here").unwrap());
        i.on_ack();
        i.expire_timer_i();
        layer.invite_server_txns.insert("timer-i".to_string(), i);

        // J and K retain their respective UDP transactions for absorption.
        let mut j = NonInviteServerTransaction::new(
            options.clone(), remote, "timer-j".to_string());
        j.send_final(options.create_response(200, "OK").unwrap());
        j.expire_timer_j();
        layer.non_invite_server_txns.insert("timer-j".to_string(), j);

        let mut k = NonInviteClientTransaction::new(
            options.clone(), remote, "timer-k".to_string());
        k.on_response(options.create_response(200, "OK").unwrap());
        k.expire_timer_k();
        layer.non_invite_client_txns.insert("timer-k".to_string(), k);

        let events = layer.expire_timers();
        let mut timeout_branches: Vec<&str> = events.iter()
            .map(|event| event.branch.as_str()).collect();
        timeout_branches.sort_unstable();
        assert_eq!(timeout_branches, vec!["timer-b", "timer-f", "timer-h"]);
        assert_eq!(layer.invite_client_txns["timer-b"].state,
            InviteClientState::Terminated);
        assert_eq!(layer.invite_client_txns["timer-d"].state,
            InviteClientState::Terminated);
        assert_eq!(layer.invite_server_txns["timer-h"].state,
            InviteServerState::Terminated);
        assert_eq!(layer.invite_server_txns["timer-i"].state,
            InviteServerState::Terminated);
        assert_eq!(layer.non_invite_client_txns["timer-f"].state,
            NonInviteClientState::Terminated);
        assert_eq!(layer.non_invite_client_txns["timer-k"].state,
            NonInviteClientState::Terminated);
        assert_eq!(layer.non_invite_server_txns["timer-j"].state,
            NonInviteServerState::Terminated);
    }

    #[tokio::test]
    async fn test_non_2xx_invite_response_generates_transaction_ack() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();
        let branch = "z9hG4bKclient-non2xx";
        let invite = build_invite_request("client-non2xx", branch);

        stack.send_invite(invite.clone(), peer_addr).await.unwrap();
        let wire_invite = recv_peer(&peer).await;
        assert_eq!(wire_invite.method(), Some(SipMethod::Invite));
        let mut busy = invite.create_response(486, "Busy Here").unwrap();
        busy.headers.iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case(header_names::TO))
            .unwrap().value.push_str(";tag=busy-peer");
        stack.handle_response(busy.clone(), peer_addr).await;

        let ack = recv_peer(&peer).await;
        assert_eq!(ack.method(), Some(SipMethod::Ack));
        assert_eq!(TransactionLayer::extract_branch(&ack).as_deref(), Some(branch));
        assert_eq!(ack.get_header(header_names::CSEQ), Some("1 ACK"));
        assert_eq!(ack.to_header(), busy.to_header());
        assert!(matches!(rx.recv().await, Some(SipEvent::Response { .. })));
    }

    /// A CANCEL matching no INVITE transaction gets 481 Call/Transaction
    /// Does Not Exist (RFC 3261 §9.2) and no application event.
    #[tokio::test]
    async fn test_cancel_unknown_transaction_gets_481() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut stack = SipStack::new(addr).await.unwrap();
        let mut rx = stack.take_event_rx().unwrap();

        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();

        stack
            .handle_request(
                build_cancel_request("cancel-55-unknown", "z9hG4bKnosuch"),
                peer_addr,
            )
            .await;

        let resp = recv_peer(&peer).await;
        assert_eq!(
            resp.status_code(),
            Some(481),
            "CANCEL with no matching INVITE must get 481"
        );
        assert!(
            rx.try_recv().is_err(),
            "an unmatched CANCEL must not reach the application layer"
        );
    }
}
