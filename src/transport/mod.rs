mod types;

pub use types::{ConnectionId, Envelope, TenantId};

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransportError {
    #[error("not connected")]
    NotConnected,

    #[error("already connected")]
    AlreadyConnected,

    #[error("connection closed")]
    ConnectionClosed,

    #[error("send failed: {0}")]
    SendFailed(String),
}

/// Bidirectional message transport.
///
/// Moves [`Envelope`]s between endpoints. Implementations guarantee
/// FIFO ordering within a single connection. The transport is agnostic
/// to payload semantics — signing, verification, and serialization
/// happen at higher layers.
#[allow(async_fn_in_trait)]
pub trait MessageTransport: Send + Sync {
    async fn connect(&mut self) -> Result<ConnectionId, TransportError>;
    async fn disconnect(&mut self) -> Result<(), TransportError>;
    async fn send(&self, envelope: &Envelope) -> Result<(), TransportError>;
    async fn recv(&self) -> Result<Envelope, TransportError>;
    fn is_connected(&self) -> bool;
}

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;
