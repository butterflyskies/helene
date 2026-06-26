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

    #[error("recv failed: {0}")]
    RecvFailed(String),
}

/// Bidirectional message transport.
///
/// Moves [`Envelope`]s between endpoints. Implementations guarantee
/// FIFO ordering within a single connection. The transport is agnostic
/// to payload semantics — signing, verification, and serialization
/// happen at higher layers.
///
/// # Object safety
///
/// This trait uses return-position `impl Trait` (RPITIT) in its async methods,
/// which means it cannot be used as `dyn MessageTransport`. This is deliberate:
/// transports are selected at compile time via generics, giving us static
/// dispatch and explicit `Send` bounds on every future without boxing overhead.
/// If runtime polymorphism is needed, wrap a concrete transport in an enum or
/// use a manual vtable.
pub trait MessageTransport: Send + Sync {
    fn connect(&mut self) -> impl Future<Output = Result<ConnectionId, TransportError>> + Send;

    /// Permanently tears down the connection and releases its resources.
    ///
    /// After `disconnect()`, the transport is *not* reconnectable — the
    /// underlying channel is consumed (e.g., the sender half is dropped).
    /// Calling [`connect`](Self::connect) after a disconnect returns
    /// [`TransportError::ConnectionClosed`]. If you need a fresh connection,
    /// construct a new transport instance.
    fn disconnect(&mut self) -> impl Future<Output = Result<(), TransportError>> + Send;

    fn send(&self, envelope: &Envelope) -> impl Future<Output = Result<(), TransportError>> + Send;
    fn recv(&self) -> impl Future<Output = Result<Envelope, TransportError>> + Send;
    fn is_connected(&self) -> bool;
}

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;
