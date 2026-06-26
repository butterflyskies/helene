use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Mutex, mpsc};

use super::{ConnectionId, Envelope, MessageTransport, TenantId, TransportError};

/// In-process mock transport backed by tokio mpsc channels.
///
/// Created in pairs via [`MockTransport::pair`]. Each side sends
/// to the other's receiver, providing a bidirectional pipe with
/// FIFO ordering guarantees matching the [`MessageTransport`] contract.
#[allow(dead_code)]
pub struct MockTransport {
    connected: AtomicBool,
    connection_id: ConnectionId,
    tenant_id: TenantId,
    tx: Option<mpsc::Sender<Envelope>>,
    rx: Mutex<mpsc::Receiver<Envelope>>,
}

impl MockTransport {
    /// The tenant this transport belongs to.
    #[allow(dead_code)]
    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    /// Create a connected pair of mock transports.
    ///
    /// Messages sent by one side are received by the other.
    /// Both sides start in the disconnected state — call
    /// [`MessageTransport::connect`] before sending.
    pub fn pair(tenant_id: TenantId) -> (Self, Self) {
        let (tx_a, rx_a) = mpsc::channel(256);
        let (tx_b, rx_b) = mpsc::channel(256);
        (
            Self {
                connected: AtomicBool::new(false),
                connection_id: ConnectionId("mock-a".into()),
                tenant_id: tenant_id.clone(),
                tx: Some(tx_a),
                rx: Mutex::new(rx_b),
            },
            Self {
                connected: AtomicBool::new(false),
                connection_id: ConnectionId("mock-b".into()),
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
