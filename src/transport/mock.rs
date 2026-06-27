use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Mutex, mpsc};

use super::{ConnectionId, Envelope, MessageTransport, TenantId, TransportError};

/// Default buffer capacity (messages per direction) for mock transport channels.
///
/// Sized generously so tests exercising normal send/recv flow never hit
/// backpressure incidentally; tests that *want* to observe blocking opt into a
/// smaller capacity via [`MockTransport::pair_with_buffer`].
const DEFAULT_BUFFER_SIZE: usize = 256;

/// In-process mock transport backed by tokio mpsc channels.
///
/// Created in pairs via [`MockTransport::pair`] or [`MockTransport::pair_with_buffer`].
/// Each side sends to the other's receiver, providing a bidirectional pipe with
/// FIFO ordering guarantees matching the [`MessageTransport`] contract.
pub struct MockTransport {
    connected: AtomicBool,
    connection_id: ConnectionId,
    tenant_id: TenantId,
    tx: Option<mpsc::Sender<Envelope>>,
    rx: Mutex<mpsc::Receiver<Envelope>>,
}

impl MockTransport {
    /// The tenant this transport belongs to.
    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    /// Create a connected pair of mock transports with the default buffer
    /// size (`DEFAULT_BUFFER_SIZE` messages per direction).
    ///
    /// Messages sent by one side are received by the other.
    /// Both sides start in the disconnected state — call
    /// [`MessageTransport::connect`] before sending.
    pub fn pair(tenant_id: TenantId) -> (Self, Self) {
        Self::pair_with_buffer(tenant_id, DEFAULT_BUFFER_SIZE)
    }

    /// Create a connected pair of mock transports with a custom buffer
    /// capacity.
    ///
    /// `buffer_size` controls how many messages can be buffered per direction
    /// before the sender blocks (backpressure). The default via [`pair`](Self::pair)
    /// is `DEFAULT_BUFFER_SIZE`, which is generous for testing. Use a smaller
    /// value to exercise backpressure behavior in tests.
    pub fn pair_with_buffer(tenant_id: TenantId, buffer_size: usize) -> (Self, Self) {
        let (tx_a, rx_a) = mpsc::channel(buffer_size);
        let (tx_b, rx_b) = mpsc::channel(buffer_size);
        (
            Self {
                connected: AtomicBool::new(false),
                connection_id: ConnectionId::from("mock-a"),
                tenant_id: tenant_id.clone(),
                tx: Some(tx_a),
                rx: Mutex::new(rx_b),
            },
            Self {
                connected: AtomicBool::new(false),
                connection_id: ConnectionId::from("mock-b"),
                tenant_id,
                tx: Some(tx_b),
                rx: Mutex::new(rx_a),
            },
        )
    }
}

impl MessageTransport for MockTransport {
    async fn connect(&mut self) -> Result<ConnectionId, TransportError> {
        if self.connected.load(Ordering::Relaxed) {
            return Err(TransportError::AlreadyConnected);
        }
        if self.tx.is_none() {
            return Err(TransportError::ConnectionClosed);
        }
        self.connected.store(true, Ordering::Relaxed);
        Ok(self.connection_id.clone())
    }

    async fn disconnect(&mut self) -> Result<(), TransportError> {
        if !self.connected.load(Ordering::Relaxed) {
            return Err(TransportError::NotConnected);
        }
        self.tx.take();
        self.connected.store(false, Ordering::Relaxed);
        Ok(())
    }

    async fn send(&self, envelope: &Envelope) -> Result<(), TransportError> {
        if !self.connected.load(Ordering::Relaxed) {
            return Err(TransportError::NotConnected);
        }
        let tx = self.tx.as_ref().ok_or(TransportError::ConnectionClosed)?;
        tx.send(envelope.clone())
            .await
            .map_err(|e| TransportError::SendFailed(e.to_string()))
    }

    async fn recv(&self) -> Result<Envelope, TransportError> {
        if !self.connected.load(Ordering::Relaxed) {
            return Err(TransportError::NotConnected);
        }
        let mut rx = self.rx.lock().await;
        rx.recv().await.ok_or(TransportError::ConnectionClosed)
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}
