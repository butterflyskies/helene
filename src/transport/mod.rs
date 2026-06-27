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
    /// Establishes the connection and returns its [`ConnectionId`].
    ///
    /// Must be called once before [`send`](Self::send) or [`recv`](Self::recv);
    /// both fail with [`TransportError::NotConnected`] until the connection is
    /// live. A transport may only be connected once: it is not reusable across
    /// a [`disconnect`](Self::disconnect).
    ///
    /// # Errors
    ///
    /// - [`TransportError::AlreadyConnected`] if the transport is already
    ///   connected.
    /// - [`TransportError::ConnectionClosed`] if the transport has already been
    ///   disconnected — reconnection is not supported; construct a new instance
    ///   instead.
    fn connect(&mut self) -> impl Future<Output = Result<ConnectionId, TransportError>> + Send;

    /// Permanently tears down the connection and releases its resources.
    ///
    /// After `disconnect()`, the transport is *not* reconnectable — the
    /// underlying channel is consumed (e.g., the sender half is dropped).
    /// Calling [`connect`](Self::connect) after a disconnect returns
    /// [`TransportError::ConnectionClosed`]. If you need a fresh connection,
    /// construct a new transport instance.
    fn disconnect(&mut self) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Sends an `envelope` to the peer.
    ///
    /// Takes `&self`, so multiple sends may be issued concurrently from shared
    /// references; the transport preserves FIFO ordering between the peer's
    /// receives. The returned future resolves once the envelope is accepted into
    /// the transport's outbound buffer — under backpressure (a full buffer) it
    /// awaits until space is available rather than dropping the message.
    ///
    /// # Errors
    ///
    /// - [`TransportError::NotConnected`] if [`connect`](Self::connect) has not
    ///   been called, or the transport has been disconnected.
    /// - [`TransportError::ConnectionClosed`] if the underlying channel has been
    ///   torn down.
    /// - [`TransportError::SendFailed`] if the peer's receiving end is gone and
    ///   the envelope cannot be delivered.
    fn send(&self, envelope: &Envelope) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Receives the next [`Envelope`] from the peer.
    ///
    /// Takes `&self` and awaits until a message is available, returning envelopes
    /// in the FIFO order the peer sent them. Buffered messages remain drainable
    /// after the peer disconnects; only once the buffer is empty *and* the peer
    /// has closed the connection does this report [`TransportError::ConnectionClosed`].
    ///
    /// # Errors
    ///
    /// - [`TransportError::NotConnected`] if [`connect`](Self::connect) has not
    ///   been called on this transport, or it has been disconnected locally.
    /// - [`TransportError::ConnectionClosed`] if the peer has closed the
    ///   connection and no further buffered messages remain.
    fn recv(&self) -> impl Future<Output = Result<Envelope, TransportError>> + Send;

    /// Returns whether the transport is currently connected.
    ///
    /// Synchronous and non-blocking. Returns `false` before
    /// [`connect`](Self::connect) succeeds and after
    /// [`disconnect`](Self::disconnect); `true` while the connection is live.
    fn is_connected(&self) -> bool;
}

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;
