mod types;

pub use types::{ConnectionId, Envelope};

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

    #[error("receive failed: {0}")]
    ReceiveFailed(String),
}

/// Bidirectional message transport.
///
/// Moves [`Envelope`]s between endpoints. Implementations guarantee
/// FIFO ordering within a single connection. The transport is agnostic
/// to payload semantics — signing, verification, and serialization
/// happen at higher layers.
pub trait MessageTransport: Send + Sync {
    /// Establish the transport connection. Returns the connection's
    /// identifier on success, or [`TransportError::AlreadyConnected`]
    /// if a connection is already active.
    fn connect(
        &mut self,
    ) -> impl std::future::Future<Output = Result<ConnectionId, TransportError>> + Send;

    /// Tear down the transport connection. Pending receives on the
    /// remote side will observe [`TransportError::ConnectionClosed`].
    fn disconnect(
        &mut self,
    ) -> impl std::future::Future<Output = Result<(), TransportError>> + Send;

    /// Send an envelope to the remote endpoint. Requires an active
    /// connection.
    fn send(
        &self,
        envelope: &Envelope,
    ) -> impl std::future::Future<Output = Result<(), TransportError>> + Send;

    /// Receive the next envelope from the remote endpoint. Blocks
    /// until a message arrives or the connection closes.
    fn recv(&self) -> impl std::future::Future<Output = Result<Envelope, TransportError>> + Send;

    /// Whether the transport currently has an active connection.
    fn is_connected(&self) -> bool;
}

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;
