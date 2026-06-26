mod host;
mod types;

pub use host::InMemorySessionHost;
pub use types::{ProcessResult, SessionConfig, SessionHealth, SessionId};

use std::future::Future;
use thiserror::Error;

use crate::provider::CompletionResponse;
use crate::transport::TenantId;
use crate::verify::{Message, SignedMessage};

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("session not found for tenant: {0}")]
    TenantNotFound(TenantId),

    #[error("transport error: {0}")]
    Transport(#[from] crate::transport::TransportError),

    #[error("verification failed: {0}")]
    Verification(#[from] crate::verify::VerifyError),

    #[error("provider error: {0}")]
    Provider(#[from] crate::provider::ProviderError),

    #[error("tenant already exists: {0}")]
    TenantAlreadyExists(TenantId),
}

/// Orchestrates the verified message lifecycle.
///
/// The session host is the glue layer — it consumes [`MessageVerifier`],
/// [`MessageTransport`], and [`InferenceProvider`] to drive the full
/// pipeline: receive envelope → verify → build context → complete → respond.
///
/// Each tenant gets an isolated session with its own context, HMAC key,
/// and provider configuration. The session host routes by [`TenantId`].
pub trait SessionHost: Send + Sync {
    /// Process a verified message through the inference pipeline.
    ///
    /// The full lifecycle:
    /// 1. Verify the message signature
    /// 2. Append to the tenant's conversation context
    /// 3. Build a completion request
    /// 4. Route to the inference provider
    /// 5. Return the response
    fn process_message(
        &self,
        tenant_id: &TenantId,
        message: Message,
    ) -> impl Future<Output = Result<CompletionResponse, SessionError>> + Send;

    /// Register a new tenant session with the given configuration.
    fn register_tenant(
        &self,
        config: SessionConfig,
    ) -> impl Future<Output = Result<SessionId, SessionError>> + Send;

    /// Remove a tenant session.
    fn remove_tenant(
        &self,
        tenant_id: &TenantId,
    ) -> impl Future<Output = Result<(), SessionError>> + Send;

    /// Health check for a specific tenant's session.
    fn health(
        &self,
        tenant_id: &TenantId,
    ) -> impl Future<Output = Result<SessionHealth, SessionError>> + Send;

    /// List all registered tenant IDs.
    fn tenants(&self) -> impl Future<Output = Vec<TenantId>> + Send;

    /// Verify a signed message against the session's verifier.
    fn verify(&self, signed: &SignedMessage) -> Result<Message, SessionError>;
}

#[cfg(test)]
mod tests;
