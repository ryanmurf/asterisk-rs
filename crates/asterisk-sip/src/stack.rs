//! SIP Stack Coordinator.
//!
//! Wires together the transport, transaction, and dialog layers into a
//! running SIP stack. Provides the main event loop that drives message
//! processing from recv through transaction matching to dialog routing.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::dialog::Dialog;
use crate::parser::{SipMessage, SipMethod, header_names, extract_tag};
use crate::session::SipSession;
use crate::transaction::{
    ClientTransaction, NonInviteClientTransaction, NonInviteServerTransaction, ServerTransaction,
};
use crate::transport::{SipTransport, TransportError, UdpTransport};

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
struct TransactionLayer {
    invite_client_txns: HashMap<String, ClientTransaction>,
    invite_server_txns: HashMap<String, ServerTransaction>,
    non_invite_client_txns: HashMap<String, NonInviteClientTransaction>,
    non_invite_server_txns: HashMap<String, NonInviteServerTransaction>,
}

impl TransactionLayer {
    fn new() -> Self {
        Self {
            invite_client_txns: HashMap::new(),
            invite_server_txns: HashMap::new(),
            non_invite_client_txns: HashMap::new(),
            non_invite_server_txns: HashMap::new(),
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
    ) -> Option<String> {
        let branch = Self::extract_branch(response)?;

        if let Some(txn) = self.invite_client_txns.get_mut(&branch) {
            txn.on_response(response.clone());
            return Some(branch);
        }

        if let Some(txn) = self.non_invite_client_txns.get_mut(&branch) {
            txn.on_response(response.clone());
            return Some(branch);
        }

        None
    }

    /// Match an incoming request to a server transaction, or return None
    /// if it is a new request.
    fn match_request(&self, request: &SipMessage) -> Option<String> {
        let branch = Self::extract_branch(request)?;
        if self.invite_server_txns.contains_key(&branch) {
            return Some(branch);
        }
        if self.non_invite_server_txns.contains_key(&branch) {
            return Some(branch);
        }
        None
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

    /// Collect timed-out transaction branches.
    fn timed_out_transactions(&self) -> Vec<String> {
        let mut result = Vec::new();
        for (branch, txn) in &self.invite_client_txns {
            if txn.is_timed_out() {
                result.push(branch.clone());
            }
        }
        for (branch, txn) in &self.non_invite_client_txns {
            if txn.is_timed_out() {
                result.push(branch.clone());
            }
        }
        for (branch, txn) in &self.invite_server_txns {
            if txn.is_timed_out() {
                result.push(branch.clone());
            }
        }
        result
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

/// The SIP stack coordinator: wires transport, transactions, and dialogs.
pub struct SipStack {
    transport: Arc<UdpTransport>,
    transaction_layer: Arc<RwLock<TransactionLayer>>,
    #[allow(dead_code)]
    dialog_manager: Arc<RwLock<DialogManager>>,
    local_addr: SocketAddr,
    event_tx: mpsc::Sender<SipEvent>,
    event_rx: Option<mpsc::Receiver<SipEvent>>,
}

impl SipStack {
    /// Create a new SIP stack bound to the given address.
    pub async fn new(bind_addr: SocketAddr) -> Result<Self, TransportError> {
        let transport = UdpTransport::bind(bind_addr).await?;
        let local_addr = transport.local_addr()?;
        let (event_tx, event_rx) = mpsc::channel(256);

        info!(addr = %local_addr, "SIP stack created");

        Ok(Self {
            transport: Arc::new(transport),
            transaction_layer: Arc::new(RwLock::new(TransactionLayer::new())),
            dialog_manager: Arc::new(RwLock::new(DialogManager::new())),
            local_addr,
            event_tx,
            event_rx: Some(event_rx),
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
        self.transport.send(&response, remote_addr).await
    }

    /// Get a clone of the transport for use by external components
    /// (e.g., the event handler needs to send SIP responses).
    pub fn transport(&self) -> Arc<UdpTransport> {
        self.transport.clone()
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
        let branch = {
            let mut txn_layer = self.transaction_layer.write();
            txn_layer.process_response(&response)
        };

        if branch.is_some() {
            debug!(src = %src, "Response matched transaction");
        } else {
            debug!(src = %src, "Response did not match any transaction (stray)");
        }

        // Emit event for application layer
        let _ = self.event_tx.try_send(SipEvent::Response {
            response,
            remote_addr: src,
        });
    }

    /// Handle an incoming request.
    async fn handle_request(&self, request: SipMessage, src: SocketAddr) {
        let method = match request.method() {
            Some(m) => m,
            None => return,
        };

        // Check for retransmission (existing server transaction)
        // Extract the response to retransmit (if any) without holding the lock across await.
        let retransmit_response;
        {
            let txn_layer = self.transaction_layer.read();
            retransmit_response = if let Some(branch) = txn_layer.match_request(&request) {
                debug!(branch = %branch, "Retransmission of existing request");
                let resp = txn_layer
                    .invite_server_txns
                    .get(&branch)
                    .and_then(|txn| txn.last_response.clone());
                Some(resp)
            } else {
                None
            };
            drop(txn_layer);
        }

        if let Some(maybe_resp) = retransmit_response {
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
                    let _ = self.event_tx.try_send(SipEvent::IncomingInvite {
                        session,
                        request,
                        remote_addr: src,
                    });
                }
            }
            SipMethod::Ack => {
                // Route ACK to the matching INVITE server transaction
                if let Some(branch) = TransactionLayer::extract_branch(&request) {
                    let mut txn_layer = self.transaction_layer.write();
                    if let Some(txn) = txn_layer.invite_server_txns.get_mut(&branch) {
                        txn.on_ack();
                        debug!(branch = %branch, "ACK received for INVITE server transaction");
                    }
                }
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
                let _ = self.event_tx.try_send(SipEvent::IncomingBye {
                    call_id,
                    request,
                    remote_addr: src,
                });
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

                let _ = self.event_tx.try_send(SipEvent::IncomingRequest {
                    request,
                    remote_addr: src,
                });
            }
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

        // Collect timed-out transactions
        let timed_out: Vec<String> = {
            let txn_layer = self.transaction_layer.read();
            txn_layer.timed_out_transactions()
        };

        for branch in timed_out {
            warn!(branch = %branch, "Transaction timed out");
            let mut txn_layer = self.transaction_layer.write();
            if let Some(txn) = txn_layer.invite_client_txns.get_mut(&branch) {
                txn.terminate();
            }
            if let Some(txn) = txn_layer.non_invite_client_txns.get_mut(&branch) {
                txn.terminate();
            }
            if let Some(txn) = txn_layer.invite_server_txns.get_mut(&branch) {
                txn.terminate();
            }
            let _ = self.event_tx.try_send(SipEvent::TransactionTimeout {
                branch,
            });
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
}
