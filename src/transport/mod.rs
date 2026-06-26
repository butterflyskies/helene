mod types;

pub use types::{ConnectionId, Envelope, TenantId};

use std::future::Future;

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
pub trait MessageTransport: Send + Sync {
    fn connect(&mut self) -> impl Future<Output = Result<ConnectionId, TransportError>> + Send;
    fn disconnect(&mut self) -> impl Future<Output = Result<(), TransportError>> + Send;
    fn send(&self, envelope: &Envelope) -> impl Future<Output = Result<(), TransportError>> + Send;
    fn recv(&self) -> impl Future<Output = Result<Envelope, TransportError>> + Send;
    fn is_connected(&self) -> bool;
}

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;
