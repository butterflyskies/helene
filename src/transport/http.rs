//! Streamable HTTP transport for MCP.
//!
//! Implements [`MessageTransport`] over HTTP POST (sending) and
//! Server-Sent Events (receiving). Follows the MCP Streamable HTTP
//! protocol:
//!
//! - **Send:** POST JSON-encoded [`Envelope`]s to the endpoint.
//! - **Receive:** GET with `Accept: text/event-stream` opens an SSE
//!   stream; each `message` event carries a JSON-encoded [`Envelope`].
//! - **Session management:** The server may return an `Mcp-Session-Id`
//!   header; subsequent requests include it automatically.
//! - **Reconnection:** The SSE listener reconnects with exponential
//!   backoff on transient failures.
//! - **Integrity:** When an HMAC key is configured, outgoing POST
//!   bodies are signed via `X-Signature` (hex-encoded HMAC-SHA256)
//!   and incoming SSE event data is verified against the same header
//!   delivered as a field in the SSE event.
//!
//! # Known gaps
//!
//! - No `Last-Event-ID` resumption (SSE spec §9.2.4). Reconnection
//!   replays from the current stream position.
//! - DELETE on disconnect is best-effort; network failures are logged
//!   but not surfaced.
//! - No HTTP/2 or WebSocket upgrade path.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use std::fmt::Write as _;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use url::Url;

use super::{ConnectionId, Envelope, MessageTransport, TenantId, TransportError};

type HmacSha256 = Hmac<Sha256>;

// ── Wire format ─────────────────────────────────────────────────

/// JSON representation of an [`Envelope`] on the wire.
///
/// The opaque `payload` is base64-encoded to survive JSON
/// round-tripping without data loss.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WireEnvelope {
    tenant_id: String,
    seq: u64,
    /// Base64-encoded payload bytes.
    payload: String,
}

impl WireEnvelope {
    fn from_envelope(env: &Envelope) -> Self {
        Self {
            tenant_id: env.tenant_id.0.clone(),
            seq: env.seq,
            payload: BASE64.encode(&env.payload),
        }
    }

    fn into_envelope(self) -> Result<Envelope, TransportError> {
        let payload = BASE64
            .decode(&self.payload)
            .map_err(|e| TransportError::RecvFailed(format!("invalid base64 payload: {e}")))?;
        Ok(Envelope {
            tenant_id: TenantId(self.tenant_id),
            seq: self.seq,
            payload,
        })
    }
}

// ── HMAC signing ────────────────────────────────────────────────

/// Transport-level HMAC-SHA256 signer for request integrity.
///
/// Operates on raw bytes (the serialized JSON body or SSE event
/// data), independent of the higher-level [`MessageVerifier`] which
/// signs domain-specific [`Message`] structs.
struct TransportSigner {
    key: zeroize::Zeroizing<Vec<u8>>,
}

impl TransportSigner {
    fn new(key: Vec<u8>) -> Self {
        Self {
            key: zeroize::Zeroizing::new(key),
        }
    }

    fn sign(&self, data: &[u8]) -> Vec<u8> {
        let mut mac =
            HmacSha256::new_from_slice(&self.key).expect("HMAC-SHA256 accepts any key length");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    fn verify(&self, data: &[u8], signature: &[u8]) -> bool {
        let expected = self.sign(data);
        if expected.len() != signature.len() {
            return false;
        }
        expected.ct_eq(signature).into()
    }

    fn sign_hex(&self, data: &[u8]) -> String {
        hex_encode(&self.sign(data))
    }

    fn verify_hex(&self, data: &[u8], hex_sig: &str) -> bool {
        let Some(sig_bytes) = hex_decode(hex_sig) else {
            return false;
        };
        self.verify(data, &sig_bytes)
    }
}

impl fmt::Debug for TransportSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransportSigner")
            .field("key", &"[REDACTED]")
            .finish()
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").unwrap();
    }
    s
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

// ── Configuration ───────────────────────────────────────────────

/// Configuration for [`HttpTransport`].
#[derive(Debug, Clone)]
pub struct HttpTransportConfig {
    /// MCP server endpoint URL.
    pub endpoint: Url,
    /// Tenant identifier for this transport.
    pub tenant_id: TenantId,
    /// Optional HMAC key for transport-level request signing.
    /// When set, outgoing POSTs include an `X-Signature` header and
    /// incoming SSE events are verified against a `signature` field.
    ///
    /// Consumed (moved) during [`HttpTransport::new`] so the key
    /// does not linger in config memory.
    pub hmac_key: Option<zeroize::Zeroizing<Vec<u8>>>,
    /// Maximum number of SSE reconnection attempts before giving up.
    pub reconnect_max_retries: u32,
    /// Initial delay between reconnection attempts.
    pub reconnect_base_delay: Duration,
    /// Maximum delay between reconnection attempts (caps backoff).
    pub reconnect_max_delay: Duration,
    /// Timeout for individual HTTP requests.
    pub request_timeout: Duration,
    /// Buffer size for the internal receive channel.
    pub recv_buffer: usize,
    /// Maximum bytes in a single SSE line before the connection is
    /// aborted (protects against OOM from malicious servers).
    pub max_line_bytes: usize,
    /// Maximum bytes accumulated in a single SSE event's data
    /// fields before the connection is aborted.
    pub max_event_data_bytes: usize,
}

impl HttpTransportConfig {
    /// Minimal config with only the required fields; everything
    /// else gets production-sensible defaults.
    pub fn new(endpoint: Url, tenant_id: TenantId) -> Self {
        Self {
            endpoint,
            tenant_id,
            hmac_key: None,
            reconnect_max_retries: 5,
            reconnect_base_delay: Duration::from_millis(500),
            reconnect_max_delay: Duration::from_secs(30),
            request_timeout: Duration::from_secs(30),
            recv_buffer: 256,
            max_line_bytes: 1024 * 1024,           // 1 MiB
            max_event_data_bytes: 4 * 1024 * 1024, // 4 MiB
        }
    }

    /// Set the HMAC key for transport-level signing.
    #[must_use]
    pub fn with_hmac_key(mut self, key: impl Into<Vec<u8>>) -> Self {
        self.hmac_key = Some(zeroize::Zeroizing::new(key.into()));
        self
    }
}

// ── SSE parser ──────────────────────────────────────────────────

/// Minimal SSE event parsed from the byte stream.
#[derive(Debug, Default)]
struct SseEvent {
    event: Option<String>,
    data: String,
    /// Optional signature field (custom extension for HMAC).
    signature: Option<String>,
}

/// Incremental SSE line parser.
///
/// Accumulates lines until a blank line signals the end of an event.
struct SseParser {
    current: SseEvent,
    has_data: bool,
}

impl SseParser {
    fn new() -> Self {
        Self {
            current: SseEvent::default(),
            has_data: false,
        }
    }

    /// Feed a single line (without trailing newline). Returns `Some`
    /// when a blank line completes an event that has data.
    fn feed_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            // Blank line = event boundary.
            if self.has_data {
                let event = std::mem::take(&mut self.current);
                self.has_data = false;
                return Some(event);
            }
            return None;
        }

        // Comment lines start with ':'
        if line.starts_with(':') {
            return None;
        }

        let (field, value) = match line.find(':') {
            Some(pos) => {
                let value = &line[pos + 1..];
                // Strip single leading space per SSE spec.
                let value = value.strip_prefix(' ').unwrap_or(value);
                (&line[..pos], value)
            }
            None => (line, ""),
        };

        match field {
            "event" => self.current.event = Some(value.to_owned()),
            "data" => {
                if self.has_data {
                    self.current.data.push('\n');
                }
                self.current.data.push_str(value);
                self.has_data = true;
            }
            "signature" => self.current.signature = Some(value.to_owned()),
            _ => {} // Ignore unknown fields per spec.
        }

        None
    }
}

// ── Transport ───────────────────────────────────────────────────

/// Streamable HTTP transport for MCP.
///
/// See [module-level documentation](self) for protocol details.
pub struct HttpTransport {
    config: HttpTransportConfig,
    client: Client,
    connected: AtomicBool,
    session_id: Arc<Mutex<Option<String>>>,
    /// Receives envelopes produced by the SSE background task.
    rx: Mutex<Option<mpsc::Receiver<Result<Envelope, TransportError>>>>,
    /// Handle to the SSE listener; aborted on disconnect.
    sse_handle: Mutex<Option<JoinHandle<()>>>,
    /// Signer is behind Arc so the SSE task can share it.
    signer: Option<Arc<TransportSigner>>,
}

impl fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpTransport")
            .field("endpoint", &self.config.endpoint)
            .field("tenant_id", &self.config.tenant_id)
            .field("connected", &self.connected.load(Ordering::Relaxed))
            .field("has_hmac", &self.signer.is_some())
            .finish()
    }
}

impl HttpTransport {
    /// Create a new transport from configuration.
    ///
    /// The transport starts disconnected — call
    /// [`connect`](MessageTransport::connect) before sending.
    ///
    /// The HMAC key is moved out of `config` so it does not
    /// linger in the caller's copy.
    pub fn new(mut config: HttpTransportConfig) -> Self {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .expect("failed to build reqwest client");

        // Clone the key out of the Zeroizing wrapper. The config's
        // Zeroizing<Vec<u8>> will zeroize its copy when dropped;
        // TransportSigner's internal Zeroizing zeroizes the signer's copy.
        let signer = config
            .hmac_key
            .take()
            .map(|k| Arc::new(TransportSigner::new((*k).clone())));

        Self {
            config,
            client,
            connected: AtomicBool::new(false),
            session_id: Arc::new(Mutex::new(None)),
            rx: Mutex::new(None),
            sse_handle: Mutex::new(None),
            signer,
        }
    }

    /// Spawn the SSE listener background task.
    fn spawn_sse_listener(
        &self,
        tx: mpsc::Sender<Result<Envelope, TransportError>>,
    ) -> JoinHandle<()> {
        let client = self.client.clone();
        let url = self.config.endpoint.clone();
        let session_id = self.session_id.clone();
        let signer = self.signer.clone();
        let max_retries = self.config.reconnect_max_retries;
        let base_delay = self.config.reconnect_base_delay;
        let max_delay = self.config.reconnect_max_delay;
        let max_line_bytes = self.config.max_line_bytes;
        let max_event_data_bytes = self.config.max_event_data_bytes;

        tokio::spawn(async move {
            let mut retries = 0u32;

            loop {
                let result = run_sse_stream(
                    &client,
                    &url,
                    &session_id,
                    signer.as_deref(),
                    &tx,
                    max_line_bytes,
                    max_event_data_bytes,
                )
                .await;

                match result {
                    SseOutcome::Clean => break,
                    SseOutcome::ChannelClosed => break,
                    SseOutcome::Error(_e) => {
                        retries += 1;
                        if retries > max_retries {
                            let _ = tx
                                .send(Err(TransportError::RecvFailed(format!(
                                    "SSE reconnection failed after {max_retries} attempts"
                                ))))
                                .await;
                            break;
                        }
                        let delay = backoff_delay(retries, base_delay, max_delay);
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        })
    }
}

/// Outcome of a single SSE stream attempt.
enum SseOutcome {
    /// Stream ended cleanly (server closed it).
    Clean,
    /// The internal channel was closed (transport disconnected).
    ChannelClosed,
    /// A retryable error occurred.
    Error(String),
}

/// Run a single SSE stream connection, feeding envelopes into `tx`.
async fn run_sse_stream(
    client: &Client,
    url: &Url,
    session_id: &Arc<Mutex<Option<String>>>,
    signer: Option<&TransportSigner>,
    tx: &mpsc::Sender<Result<Envelope, TransportError>>,
    max_line_bytes: usize,
    max_event_data_bytes: usize,
) -> SseOutcome {
    let mut req = client
        .get(url.as_str())
        .header("Accept", "text/event-stream")
        .header("Cache-Control", "no-cache");

    // Attach session ID if we have one.
    if let Some(ref sid) = *session_id.lock().await {
        req = req.header("Mcp-Session-Id", sid.as_str());
    }

    let response = match req.send().await {
        Ok(r) => r,
        Err(e) => return SseOutcome::Error(format!("SSE GET failed: {e}")),
    };

    if !response.status().is_success() {
        return SseOutcome::Error(format!("SSE GET returned {}", response.status()));
    }

    // Capture session ID from response if present.
    if let Some(sid) = response.headers().get("mcp-session-id")
        && let Ok(s) = sid.to_str()
    {
        *session_id.lock().await = Some(s.to_owned());
    }

    let mut stream = response.bytes_stream();
    let mut parser = SseParser::new();
    let mut line_buf = Vec::<u8>::new();

    /// Dispatch a completed line to the parser and, if a full event
    /// is ready, process and forward it. Returns the appropriate
    /// outcome if the channel is closed or a limit is exceeded.
    async fn dispatch_line(
        line_buf: &mut Vec<u8>,
        parser: &mut SseParser,
        signer: Option<&TransportSigner>,
        tx: &mpsc::Sender<Result<Envelope, TransportError>>,
        max_event_data_bytes: usize,
    ) -> Option<SseOutcome> {
        let line = String::from_utf8_lossy(line_buf);
        if let Some(event) = parser.feed_line(&line) {
            if event.data.len() > max_event_data_bytes {
                let _ = tx
                    .send(Err(TransportError::RecvFailed(
                        "SSE event data exceeds size limit".into(),
                    )))
                    .await;
                return Some(SseOutcome::Error(
                    "SSE event data exceeds size limit".into(),
                ));
            }
            if let Some(result) = process_sse_event(&event, signer)
                && tx.send(result).await.is_err()
            {
                return Some(SseOutcome::ChannelClosed);
            }
        }
        line_buf.clear();
        None
    }

    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(c) => c,
            Err(e) => return SseOutcome::Error(format!("SSE stream read error: {e}")),
        };

        // SSE is line-oriented. Per WHATWG spec §9.2, lines may be
        // terminated by LF, CRLF, or bare CR.
        for &byte in chunk.iter() {
            if byte == b'\n' || byte == b'\r' {
                // For CRLF, the \r adds nothing (line_buf was already
                // dispatched on the previous \n or this is the \r of
                // \r\n — either way, skip empty lines from the split).
                if (!line_buf.is_empty() || byte == b'\n')
                    && let Some(outcome) =
                        dispatch_line(&mut line_buf, &mut parser, signer, tx, max_event_data_bytes)
                            .await
                {
                    return outcome;
                }
            } else {
                if line_buf.len() >= max_line_bytes {
                    return SseOutcome::Error(format!(
                        "SSE line exceeds {max_line_bytes} byte limit"
                    ));
                }
                line_buf.push(byte);
            }
        }
    }

    // Stream ended — flush any trailing data.
    if !line_buf.is_empty() {
        let _ = dispatch_line(&mut line_buf, &mut parser, signer, tx, max_event_data_bytes).await;
    }
    // Flush parser in case there's an event without trailing blank line.
    line_buf.clear();
    let _ = dispatch_line(&mut line_buf, &mut parser, signer, tx, max_event_data_bytes).await;

    SseOutcome::Clean
}

/// Parse an SSE event into an [`Envelope`], optionally verifying HMAC.
///
/// Returns `None` for non-"message" event types (e.g. keepalive,
/// ping) — these are silently skipped rather than treated as errors.
fn process_sse_event(
    event: &SseEvent,
    signer: Option<&TransportSigner>,
) -> Option<Result<Envelope, TransportError>> {
    // Only process "message" events (or events with no explicit type,
    // which default to "message" per the SSE spec). Other types like
    // "ping" or "heartbeat" are silently skipped.
    let event_type = event.event.as_deref().unwrap_or("message");
    if event_type != "message" {
        return None;
    }

    Some(process_message_event(event, signer))
}

/// Inner helper for processing a confirmed "message" event.
fn process_message_event(
    event: &SseEvent,
    signer: Option<&TransportSigner>,
) -> Result<Envelope, TransportError> {
    // Verify signature if signer is configured.
    if let Some(signer) = signer {
        let sig = event
            .signature
            .as_deref()
            .ok_or_else(|| TransportError::RecvFailed("missing signature on SSE event".into()))?;
        if !signer.verify_hex(event.data.as_bytes(), sig) {
            return Err(TransportError::RecvFailed(
                "HMAC verification failed on SSE event".into(),
            ));
        }
    }

    let wire: WireEnvelope = serde_json::from_str(&event.data)
        .map_err(|e| TransportError::RecvFailed(format!("invalid envelope JSON: {e}")))?;

    wire.into_envelope()
}

/// Compute exponential backoff delay, capped at `max`.
fn backoff_delay(retry: u32, base: Duration, max: Duration) -> Duration {
    let exp = base.saturating_mul(1u32.wrapping_shl(retry.min(20)));
    exp.min(max)
}

// ── MessageTransport impl ───────────────────────────────────────

impl MessageTransport for HttpTransport {
    async fn connect(&mut self) -> Result<ConnectionId, TransportError> {
        if self.connected.load(Ordering::Relaxed) {
            return Err(TransportError::AlreadyConnected);
        }

        let (tx, rx) = mpsc::channel(self.config.recv_buffer);
        let handle = self.spawn_sse_listener(tx);

        *self.rx.lock().await = Some(rx);
        *self.sse_handle.lock().await = Some(handle);

        self.connected.store(true, Ordering::Relaxed);

        let cid = ConnectionId(format!(
            "http-{}",
            self.config.endpoint.host_str().unwrap_or("unknown")
        ));
        Ok(cid)
    }

    async fn disconnect(&mut self) -> Result<(), TransportError> {
        if !self.connected.load(Ordering::Relaxed) {
            return Err(TransportError::NotConnected);
        }

        // Abort the SSE listener.
        if let Some(handle) = self.sse_handle.lock().await.take() {
            handle.abort();
        }

        // Drop the receiver so any pending sends fail.
        self.rx.lock().await.take();

        // Best-effort DELETE to terminate the session.
        if let Some(ref sid) = *self.session_id.lock().await {
            let _ = self
                .client
                .delete(self.config.endpoint.as_str())
                .header("Mcp-Session-Id", sid.as_str())
                .send()
                .await;
        }

        *self.session_id.lock().await = None;
        self.connected.store(false, Ordering::Relaxed);

        Ok(())
    }

    async fn send(&self, envelope: &Envelope) -> Result<(), TransportError> {
        if !self.connected.load(Ordering::Relaxed) {
            return Err(TransportError::NotConnected);
        }

        let wire = WireEnvelope::from_envelope(envelope);
        let body = serde_json::to_vec(&wire)
            .map_err(|e| TransportError::SendFailed(format!("serialization failed: {e}")))?;

        let mut req = self
            .client
            .post(self.config.endpoint.as_str())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        // Attach session ID.
        if let Some(ref sid) = *self.session_id.lock().await {
            req = req.header("Mcp-Session-Id", sid.as_str());
        }

        // Sign the body.
        if let Some(ref signer) = self.signer {
            req = req.header("X-Signature", signer.sign_hex(&body));
        }

        let response = req
            .body(body)
            .send()
            .await
            .map_err(|e| TransportError::SendFailed(format!("POST failed: {e}")))?;

        // Capture session ID from response.
        if let Some(sid) = response.headers().get("mcp-session-id")
            && let Ok(s) = sid.to_str()
        {
            *self.session_id.lock().await = Some(s.to_owned());
        }

        if !response.status().is_success() {
            return Err(TransportError::SendFailed(format!(
                "server returned {}",
                response.status()
            )));
        }

        Ok(())
    }

    async fn recv(&self) -> Result<Envelope, TransportError> {
        if !self.connected.load(Ordering::Relaxed) {
            return Err(TransportError::NotConnected);
        }

        let mut rx_guard = self.rx.lock().await;
        let rx = rx_guard.as_mut().ok_or(TransportError::ConnectionClosed)?;

        match rx.recv().await {
            Some(Ok(env)) => Ok(env),
            Some(Err(e)) => Err(e),
            None => Err(TransportError::ConnectionClosed),
        }
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── SSE parser unit tests ───────────────────────────────────

    #[test]
    fn sse_parser_simple_message() {
        let mut parser = SseParser::new();
        assert!(parser.feed_line("data: hello").is_none());
        let event = parser.feed_line("").unwrap();
        assert_eq!(event.data, "hello");
        assert!(event.event.is_none());
    }

    #[test]
    fn sse_parser_named_event() {
        let mut parser = SseParser::new();
        parser.feed_line("event: message");
        parser.feed_line("data: {\"test\": true}");
        let event = parser.feed_line("").unwrap();
        assert_eq!(event.event.as_deref(), Some("message"));
        assert_eq!(event.data, "{\"test\": true}");
    }

    #[test]
    fn sse_parser_multiline_data() {
        let mut parser = SseParser::new();
        parser.feed_line("data: line1");
        parser.feed_line("data: line2");
        parser.feed_line("data: line3");
        let event = parser.feed_line("").unwrap();
        assert_eq!(event.data, "line1\nline2\nline3");
    }

    #[test]
    fn sse_parser_comment_ignored() {
        let mut parser = SseParser::new();
        parser.feed_line(": this is a comment");
        parser.feed_line("data: actual data");
        let event = parser.feed_line("").unwrap();
        assert_eq!(event.data, "actual data");
    }

    #[test]
    fn sse_parser_empty_blank_lines_ignored() {
        let mut parser = SseParser::new();
        assert!(parser.feed_line("").is_none());
        assert!(parser.feed_line("").is_none());
    }

    #[test]
    fn sse_parser_signature_field() {
        let mut parser = SseParser::new();
        parser.feed_line("data: payload");
        parser.feed_line("signature: abc123");
        let event = parser.feed_line("").unwrap();
        assert_eq!(event.data, "payload");
        assert_eq!(event.signature.as_deref(), Some("abc123"));
    }

    #[test]
    fn sse_parser_unknown_field_ignored() {
        let mut parser = SseParser::new();
        parser.feed_line("custom: ignored");
        parser.feed_line("data: kept");
        let event = parser.feed_line("").unwrap();
        assert_eq!(event.data, "kept");
    }

    #[test]
    fn sse_parser_data_no_space_after_colon() {
        let mut parser = SseParser::new();
        parser.feed_line("data:nospace");
        let event = parser.feed_line("").unwrap();
        assert_eq!(event.data, "nospace");
    }

    #[test]
    fn sse_parser_consecutive_events() {
        let mut parser = SseParser::new();
        parser.feed_line("data: first");
        let e1 = parser.feed_line("").unwrap();
        assert_eq!(e1.data, "first");

        parser.feed_line("data: second");
        let e2 = parser.feed_line("").unwrap();
        assert_eq!(e2.data, "second");
    }

    // ── Wire envelope tests ────────────────────────────────────

    #[test]
    fn wire_envelope_roundtrip() {
        let env = Envelope {
            tenant_id: TenantId("tenant-1".into()),
            seq: 42,
            payload: vec![0, 1, 2, 255],
        };
        let wire = WireEnvelope::from_envelope(&env);
        let json = serde_json::to_string(&wire).unwrap();
        let parsed: WireEnvelope = serde_json::from_str(&json).unwrap();
        let recovered = parsed.into_envelope().unwrap();
        assert_eq!(recovered, env);
    }

    #[test]
    fn wire_envelope_empty_payload() {
        let env = Envelope {
            tenant_id: TenantId("t".into()),
            seq: 0,
            payload: vec![],
        };
        let wire = WireEnvelope::from_envelope(&env);
        let recovered =
            serde_json::from_str::<WireEnvelope>(&serde_json::to_string(&wire).unwrap())
                .unwrap()
                .into_envelope()
                .unwrap();
        assert_eq!(recovered, env);
    }

    #[test]
    fn wire_envelope_invalid_base64() {
        let wire = WireEnvelope {
            tenant_id: "t".into(),
            seq: 0,
            payload: "not!valid!base64!!!".into(),
        };
        let err = wire.into_envelope().unwrap_err();
        assert!(matches!(err, TransportError::RecvFailed(_)));
    }

    // ── Transport signer tests ─────────────────────────────────

    #[test]
    fn signer_roundtrip() {
        let signer = TransportSigner::new(b"secret-key".to_vec());
        let data = b"hello world";
        let sig = signer.sign(data);
        assert!(signer.verify(data, &sig));
    }

    #[test]
    fn signer_hex_roundtrip() {
        let signer = TransportSigner::new(b"key".to_vec());
        let data = b"test payload";
        let hex_sig = signer.sign_hex(data);
        assert!(signer.verify_hex(data, &hex_sig));
    }

    #[test]
    fn signer_wrong_key_fails() {
        let signer_a = TransportSigner::new(b"key-a".to_vec());
        let signer_b = TransportSigner::new(b"key-b".to_vec());
        let data = b"data";
        let sig = signer_a.sign(data);
        assert!(!signer_b.verify(data, &sig));
    }

    #[test]
    fn signer_tampered_data_fails() {
        let signer = TransportSigner::new(b"key".to_vec());
        let sig = signer.sign(b"original");
        assert!(!signer.verify(b"tampered", &sig));
    }

    #[test]
    fn signer_invalid_hex_fails() {
        let signer = TransportSigner::new(b"key".to_vec());
        assert!(!signer.verify_hex(b"data", "not-hex!"));
        assert!(!signer.verify_hex(b"data", "abc")); // odd length
    }

    #[test]
    fn signer_deterministic() {
        let signer = TransportSigner::new(b"key".to_vec());
        let data = b"same input";
        assert_eq!(signer.sign(data), signer.sign(data));
    }

    // ── Hex encoding tests ─────────────────────────────────────

    #[test]
    fn hex_roundtrip() {
        let original = vec![0x00, 0x01, 0x0f, 0x10, 0xff];
        let encoded = hex_encode(&original);
        assert_eq!(encoded, "00010f10ff");
        assert_eq!(hex_decode(&encoded).unwrap(), original);
    }

    #[test]
    fn hex_decode_odd_length() {
        assert!(hex_decode("abc").is_none());
    }

    #[test]
    fn hex_decode_invalid_chars() {
        assert!(hex_decode("zz").is_none());
    }

    // ── Backoff tests ──────────────────────────────────────────

    #[test]
    fn backoff_increases_exponentially() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(60);

        let d1 = backoff_delay(1, base, max);
        let d2 = backoff_delay(2, base, max);
        let d3 = backoff_delay(3, base, max);

        assert!(d2 > d1, "d2 ({d2:?}) should be > d1 ({d1:?})");
        assert!(d3 > d2, "d3 ({d3:?}) should be > d2 ({d2:?})");
    }

    #[test]
    fn backoff_caps_at_max() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(1);

        let d = backoff_delay(20, base, max);
        assert!(d <= max, "delay {d:?} should be <= max {max:?}");
    }

    #[test]
    fn backoff_does_not_overflow() {
        let base = Duration::from_millis(500);
        let max = Duration::from_secs(30);

        // u32::MAX retry count should not panic.
        let d = backoff_delay(u32::MAX, base, max);
        assert!(d <= max);
    }

    // ── process_sse_event tests ────────────────────────────────

    #[test]
    fn process_event_valid_no_signing() {
        let env = Envelope {
            tenant_id: TenantId("t1".into()),
            seq: 7,
            payload: vec![42],
        };
        let wire = WireEnvelope::from_envelope(&env);
        let data = serde_json::to_string(&wire).unwrap();

        let event = SseEvent {
            event: Some("message".into()),
            data,
            signature: None,
        };

        let result = process_sse_event(&event, None).unwrap().unwrap();
        assert_eq!(result, env);
    }

    #[test]
    fn process_event_default_type_is_message() {
        let env = Envelope {
            tenant_id: TenantId("t".into()),
            seq: 0,
            payload: vec![],
        };
        let wire = WireEnvelope::from_envelope(&env);

        let event = SseEvent {
            event: None, // defaults to "message"
            data: serde_json::to_string(&wire).unwrap(),
            signature: None,
        };

        assert!(process_sse_event(&event, None).is_some_and(|r| r.is_ok()));
    }

    #[test]
    fn process_event_unknown_type_skipped() {
        let event = SseEvent {
            event: Some("ping".into()),
            data: "{}".into(),
            signature: None,
        };

        // Non-"message" events are silently skipped (None), not errors.
        assert!(process_sse_event(&event, None).is_none());
    }

    #[test]
    fn process_event_invalid_json_rejected() {
        let event = SseEvent {
            event: Some("message".into()),
            data: "not json".into(),
            signature: None,
        };

        let err = process_sse_event(&event, None).unwrap().unwrap_err();
        assert!(matches!(err, TransportError::RecvFailed(_)));
    }

    #[test]
    fn process_event_hmac_valid() {
        let signer = TransportSigner::new(b"key".to_vec());
        let env = Envelope {
            tenant_id: TenantId("t".into()),
            seq: 1,
            payload: vec![1, 2, 3],
        };
        let wire = WireEnvelope::from_envelope(&env);
        let data = serde_json::to_string(&wire).unwrap();
        let sig = signer.sign_hex(data.as_bytes());

        let event = SseEvent {
            event: Some("message".into()),
            data,
            signature: Some(sig),
        };

        let result = process_sse_event(&event, Some(&signer)).unwrap().unwrap();
        assert_eq!(result, env);
    }

    #[test]
    fn process_event_hmac_missing_signature() {
        let signer = TransportSigner::new(b"key".to_vec());
        let event = SseEvent {
            event: Some("message".into()),
            data: "{}".into(),
            signature: None,
        };

        let err = process_sse_event(&event, Some(&signer))
            .unwrap()
            .unwrap_err();
        assert!(matches!(err, TransportError::RecvFailed(_)));
    }

    #[test]
    fn process_event_hmac_invalid_signature() {
        let signer = TransportSigner::new(b"key".to_vec());
        let event = SseEvent {
            event: Some("message".into()),
            data: "{\"tenant_id\":\"t\",\"seq\":0,\"payload\":\"\"}".into(),
            signature: Some("deadbeef".repeat(8)), // 64 hex chars = 32 bytes
        };

        let err = process_sse_event(&event, Some(&signer))
            .unwrap()
            .unwrap_err();
        assert!(matches!(err, TransportError::RecvFailed(_)));
    }

    // ── HttpTransport lifecycle (no server) ─────────────────────

    #[tokio::test]
    async fn not_connected_by_default() {
        let config = HttpTransportConfig::new(
            Url::parse("http://localhost:9999").unwrap(),
            TenantId("test".into()),
        );
        let transport = HttpTransport::new(config);
        assert!(!transport.is_connected());
    }

    #[tokio::test]
    async fn send_before_connect_fails() {
        let config = HttpTransportConfig::new(
            Url::parse("http://localhost:9999").unwrap(),
            TenantId("test".into()),
        );
        let transport = HttpTransport::new(config);
        let env = Envelope {
            tenant_id: TenantId("t".into()),
            seq: 0,
            payload: vec![],
        };
        assert_eq!(
            transport.send(&env).await,
            Err(TransportError::NotConnected)
        );
    }

    #[tokio::test]
    async fn recv_before_connect_fails() {
        let config = HttpTransportConfig::new(
            Url::parse("http://localhost:9999").unwrap(),
            TenantId("test".into()),
        );
        let transport = HttpTransport::new(config);
        assert_eq!(transport.recv().await, Err(TransportError::NotConnected));
    }

    #[tokio::test]
    async fn disconnect_before_connect_fails() {
        let config = HttpTransportConfig::new(
            Url::parse("http://localhost:9999").unwrap(),
            TenantId("test".into()),
        );
        let mut transport = HttpTransport::new(config);
        assert_eq!(
            transport.disconnect().await,
            Err(TransportError::NotConnected)
        );
    }

    #[tokio::test]
    async fn double_connect_fails() {
        let config = HttpTransportConfig::new(
            Url::parse("http://localhost:9999").unwrap(),
            TenantId("test".into()),
        );
        let mut transport = HttpTransport::new(config);
        transport.connect().await.unwrap();
        assert_eq!(
            transport.connect().await,
            Err(TransportError::AlreadyConnected)
        );
        // Clean up the SSE task.
        transport.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn connect_disconnect_lifecycle() {
        let config = HttpTransportConfig::new(
            Url::parse("http://localhost:9999").unwrap(),
            TenantId("test".into()),
        );
        let mut transport = HttpTransport::new(config);

        assert!(!transport.is_connected());
        let cid = transport.connect().await.unwrap();
        assert!(transport.is_connected());
        assert_eq!(cid.0, "http-localhost");

        transport.disconnect().await.unwrap();
        assert!(!transport.is_connected());
    }

    // ── Integration tests (wiremock) ────────────────────────────

    mod integration {
        use wiremock::matchers::{header, header_exists, method};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        use super::*;

        fn make_sse_body(envelopes: &[Envelope]) -> String {
            let mut body = String::new();
            for env in envelopes {
                let wire = WireEnvelope::from_envelope(env);
                let data = serde_json::to_string(&wire).unwrap();
                body.push_str(&format!("event: message\ndata: {data}\n\n"));
            }
            body
        }

        fn make_signed_sse_body(envelopes: &[Envelope], signer: &TransportSigner) -> String {
            let mut body = String::new();
            for env in envelopes {
                let wire = WireEnvelope::from_envelope(env);
                let data = serde_json::to_string(&wire).unwrap();
                let sig = signer.sign_hex(data.as_bytes());
                body.push_str(&format!(
                    "event: message\ndata: {data}\nsignature: {sig}\n\n"
                ));
            }
            body
        }

        #[tokio::test]
        async fn send_posts_to_server() {
            let server = MockServer::start().await;

            // SSE GET returns empty stream.
            Mock::given(method("GET"))
                .and(header("Accept", "text/event-stream"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string("")
                        .insert_header("content-type", "text/event-stream"),
                )
                .mount(&server)
                .await;

            // POST accepts anything.
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200))
                .expect(1)
                .mount(&server)
                .await;

            let config = HttpTransportConfig::new(
                Url::parse(&server.uri()).unwrap(),
                TenantId("test".into()),
            );
            let mut transport = HttpTransport::new(config);
            transport.connect().await.unwrap();

            let env = Envelope {
                tenant_id: TenantId("t".into()),
                seq: 1,
                payload: vec![10, 20, 30],
            };
            transport.send(&env).await.unwrap();

            transport.disconnect().await.unwrap();
        }

        #[tokio::test]
        async fn send_includes_hmac_signature_header() {
            let server = MockServer::start().await;

            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string("")
                        .insert_header("content-type", "text/event-stream"),
                )
                .mount(&server)
                .await;

            // Expect POST with X-Signature header.
            Mock::given(method("POST"))
                .and(header_exists("X-Signature"))
                .respond_with(ResponseTemplate::new(200))
                .expect(1)
                .mount(&server)
                .await;

            let config = HttpTransportConfig::new(
                Url::parse(&server.uri()).unwrap(),
                TenantId("test".into()),
            )
            .with_hmac_key(b"test-key".to_vec());

            let mut transport = HttpTransport::new(config);
            transport.connect().await.unwrap();

            let env = Envelope {
                tenant_id: TenantId("t".into()),
                seq: 0,
                payload: vec![],
            };
            transport.send(&env).await.unwrap();

            transport.disconnect().await.unwrap();
        }

        #[tokio::test]
        async fn recv_from_sse_stream() {
            let server = MockServer::start().await;

            let expected = Envelope {
                tenant_id: TenantId("tenant-1".into()),
                seq: 42,
                payload: vec![1, 2, 3, 4],
            };
            let sse_body = make_sse_body(std::slice::from_ref(&expected));

            Mock::given(method("GET"))
                .and(header("Accept", "text/event-stream"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(sse_body)
                        .insert_header("content-type", "text/event-stream"),
                )
                .mount(&server)
                .await;

            let config = HttpTransportConfig::new(
                Url::parse(&server.uri()).unwrap(),
                TenantId("test".into()),
            );
            let mut transport = HttpTransport::new(config);
            transport.connect().await.unwrap();

            let received = transport.recv().await.unwrap();
            assert_eq!(received, expected);

            transport.disconnect().await.unwrap();
        }

        #[tokio::test]
        async fn recv_multiple_envelopes_in_order() {
            let server = MockServer::start().await;

            let envelopes: Vec<Envelope> = (0..5)
                .map(|i| Envelope {
                    tenant_id: TenantId("t".into()),
                    seq: i,
                    payload: vec![i as u8],
                })
                .collect();
            let sse_body = make_sse_body(&envelopes);

            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(sse_body)
                        .insert_header("content-type", "text/event-stream"),
                )
                .mount(&server)
                .await;

            let config = HttpTransportConfig::new(
                Url::parse(&server.uri()).unwrap(),
                TenantId("test".into()),
            );
            let mut transport = HttpTransport::new(config);
            transport.connect().await.unwrap();

            for expected in &envelopes {
                let received = transport.recv().await.unwrap();
                assert_eq!(&received, expected);
            }

            transport.disconnect().await.unwrap();
        }

        #[tokio::test]
        async fn session_id_captured_from_post_response() {
            let server = MockServer::start().await;

            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string("")
                        .insert_header("content-type", "text/event-stream"),
                )
                .mount(&server)
                .await;

            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(200).insert_header("Mcp-Session-Id", "sess-abc-123"),
                )
                .mount(&server)
                .await;

            let config = HttpTransportConfig::new(
                Url::parse(&server.uri()).unwrap(),
                TenantId("test".into()),
            );
            let mut transport = HttpTransport::new(config);
            transport.connect().await.unwrap();

            let env = Envelope {
                tenant_id: TenantId("t".into()),
                seq: 0,
                payload: vec![],
            };
            transport.send(&env).await.unwrap();

            let sid = transport.session_id.lock().await;
            assert_eq!(sid.as_deref(), Some("sess-abc-123"));
            drop(sid);

            transport.disconnect().await.unwrap();
        }

        #[tokio::test]
        async fn session_id_captured_from_sse_response() {
            let server = MockServer::start().await;

            let env = Envelope {
                tenant_id: TenantId("t".into()),
                seq: 0,
                payload: vec![],
            };
            let sse_body = make_sse_body(std::slice::from_ref(&env));

            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(sse_body)
                        .insert_header("content-type", "text/event-stream")
                        .insert_header("Mcp-Session-Id", "sse-session-42"),
                )
                .mount(&server)
                .await;

            let config = HttpTransportConfig::new(
                Url::parse(&server.uri()).unwrap(),
                TenantId("test".into()),
            );
            let mut transport = HttpTransport::new(config);
            transport.connect().await.unwrap();

            // Wait for the SSE task to process at least one event.
            let _ = transport.recv().await.unwrap();

            let sid = transport.session_id.lock().await;
            assert_eq!(sid.as_deref(), Some("sse-session-42"));
            drop(sid);

            transport.disconnect().await.unwrap();
        }

        #[tokio::test]
        async fn send_failure_returns_error() {
            let server = MockServer::start().await;

            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string("")
                        .insert_header("content-type", "text/event-stream"),
                )
                .mount(&server)
                .await;

            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(500))
                .mount(&server)
                .await;

            let config = HttpTransportConfig::new(
                Url::parse(&server.uri()).unwrap(),
                TenantId("test".into()),
            );
            let mut transport = HttpTransport::new(config);
            transport.connect().await.unwrap();

            let env = Envelope {
                tenant_id: TenantId("t".into()),
                seq: 0,
                payload: vec![],
            };
            let err = transport.send(&env).await.unwrap_err();
            assert!(matches!(err, TransportError::SendFailed(_)));

            transport.disconnect().await.unwrap();
        }

        #[tokio::test]
        async fn recv_with_hmac_verification() {
            let server = MockServer::start().await;
            let hmac_key = b"integration-test-key".to_vec();
            let signer = TransportSigner::new(hmac_key.clone());

            let expected = Envelope {
                tenant_id: TenantId("signed".into()),
                seq: 7,
                payload: vec![99],
            };
            let sse_body = make_signed_sse_body(std::slice::from_ref(&expected), &signer);

            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(sse_body)
                        .insert_header("content-type", "text/event-stream"),
                )
                .mount(&server)
                .await;

            let config = HttpTransportConfig::new(
                Url::parse(&server.uri()).unwrap(),
                TenantId("test".into()),
            )
            .with_hmac_key(hmac_key);

            let mut transport = HttpTransport::new(config);
            transport.connect().await.unwrap();

            let received = transport.recv().await.unwrap();
            assert_eq!(received, expected);

            transport.disconnect().await.unwrap();
        }

        #[tokio::test]
        async fn recv_with_bad_hmac_returns_error() {
            let server = MockServer::start().await;

            // Sign with a different key than the transport uses.
            let wrong_signer = TransportSigner::new(b"wrong-key".to_vec());
            let env = Envelope {
                tenant_id: TenantId("t".into()),
                seq: 0,
                payload: vec![],
            };
            let sse_body = make_signed_sse_body(&[env], &wrong_signer);

            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(sse_body)
                        .insert_header("content-type", "text/event-stream"),
                )
                .mount(&server)
                .await;

            let config = HttpTransportConfig::new(
                Url::parse(&server.uri()).unwrap(),
                TenantId("test".into()),
            )
            .with_hmac_key(b"correct-key".to_vec());

            let mut transport = HttpTransport::new(config);
            transport.connect().await.unwrap();

            let err = transport.recv().await.unwrap_err();
            assert!(matches!(err, TransportError::RecvFailed(_)));

            transport.disconnect().await.unwrap();
        }

        #[tokio::test]
        async fn recv_with_hmac_but_missing_signature_returns_error() {
            let server = MockServer::start().await;

            // SSE body without signature fields, but transport has HMAC configured.
            let env = Envelope {
                tenant_id: TenantId("t".into()),
                seq: 0,
                payload: vec![],
            };
            let sse_body = make_sse_body(&[env]); // No signatures.

            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(sse_body)
                        .insert_header("content-type", "text/event-stream"),
                )
                .mount(&server)
                .await;

            let config = HttpTransportConfig::new(
                Url::parse(&server.uri()).unwrap(),
                TenantId("test".into()),
            )
            .with_hmac_key(b"key".to_vec());

            let mut transport = HttpTransport::new(config);
            transport.connect().await.unwrap();

            let err = transport.recv().await.unwrap_err();
            assert!(
                matches!(err, TransportError::RecvFailed(_)),
                "expected RecvFailed, got {err:?}"
            );

            transport.disconnect().await.unwrap();
        }

        #[tokio::test]
        async fn reconnection_on_sse_failure() {
            let server = MockServer::start().await;

            // First GET fails, second succeeds with an envelope.
            let env = Envelope {
                tenant_id: TenantId("t".into()),
                seq: 0,
                payload: vec![42],
            };
            let sse_body = make_sse_body(std::slice::from_ref(&env));

            // First request returns 500 (triggers reconnect).
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(500))
                .up_to_n_times(1)
                .mount(&server)
                .await;

            // Second request succeeds.
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(sse_body)
                        .insert_header("content-type", "text/event-stream"),
                )
                .mount(&server)
                .await;

            let mut config = HttpTransportConfig::new(
                Url::parse(&server.uri()).unwrap(),
                TenantId("test".into()),
            );
            config.reconnect_base_delay = Duration::from_millis(10);
            config.reconnect_max_retries = 3;

            let mut transport = HttpTransport::new(config);
            transport.connect().await.unwrap();

            // Should eventually succeed after reconnection.
            let received = tokio::time::timeout(Duration::from_secs(5), transport.recv())
                .await
                .expect("recv timed out")
                .expect("recv failed");

            assert_eq!(received, env);

            transport.disconnect().await.unwrap();
        }
    }

    // ── Proptest: wire envelope roundtrip ───────────────────────

    mod proptests {
        use proptest::prelude::*;

        use super::*;

        fn arb_envelope() -> impl Strategy<Value = Envelope> {
            (
                "[a-z0-9]{1,16}",
                any::<u64>(),
                prop::collection::vec(any::<u8>(), 0..=1024),
            )
                .prop_map(|(tid, seq, payload)| Envelope {
                    tenant_id: TenantId(tid),
                    seq,
                    payload,
                })
        }

        proptest! {
            #[test]
            fn wire_envelope_roundtrip_prop(env in arb_envelope()) {
                let wire = WireEnvelope::from_envelope(&env);
                let json = serde_json::to_string(&wire).unwrap();
                let parsed: WireEnvelope = serde_json::from_str(&json).unwrap();
                let recovered = parsed.into_envelope().unwrap();
                prop_assert_eq!(recovered, env);
            }

            #[test]
            fn signer_roundtrip_prop(
                key in prop::collection::vec(any::<u8>(), 1..=64),
                data in prop::collection::vec(any::<u8>(), 0..=4096)
            ) {
                let signer = TransportSigner::new(key);
                let sig = signer.sign(&data);
                prop_assert!(signer.verify(&data, &sig));
            }

            #[test]
            fn signer_hex_roundtrip_prop(
                key in prop::collection::vec(any::<u8>(), 1..=64),
                data in prop::collection::vec(any::<u8>(), 0..=4096)
            ) {
                let signer = TransportSigner::new(key);
                let hex = signer.sign_hex(&data);
                prop_assert!(signer.verify_hex(&data, &hex));
            }

            #[test]
            fn wrong_key_always_fails(
                key_a in prop::collection::vec(any::<u8>(), 1..=64),
                key_b in prop::collection::vec(any::<u8>(), 1..=64),
                data in prop::collection::vec(any::<u8>(), 1..=256)
            ) {
                prop_assume!(key_a != key_b);
                let signer_a = TransportSigner::new(key_a);
                let signer_b = TransportSigner::new(key_b);
                let sig = signer_a.sign(&data);
                prop_assert!(!signer_b.verify(&data, &sig));
            }

            #[test]
            fn process_event_roundtrip_prop(env in arb_envelope()) {
                let wire = WireEnvelope::from_envelope(&env);
                let data = serde_json::to_string(&wire).unwrap();
                let event = SseEvent {
                    event: Some("message".into()),
                    data,
                    signature: None,
                };
                let result = process_sse_event(&event, None).unwrap().unwrap();
                prop_assert_eq!(result, env);
            }

            #[test]
            fn process_event_with_hmac_roundtrip_prop(
                env in arb_envelope(),
                key in prop::collection::vec(any::<u8>(), 1..=64)
            ) {
                let signer = TransportSigner::new(key);
                let wire = WireEnvelope::from_envelope(&env);
                let data = serde_json::to_string(&wire).unwrap();
                let sig = signer.sign_hex(data.as_bytes());
                let event = SseEvent {
                    event: Some("message".into()),
                    data,
                    signature: Some(sig),
                };
                let result = process_sse_event(&event, Some(&signer)).unwrap().unwrap();
                prop_assert_eq!(result, env);
            }
        }
    }
}
