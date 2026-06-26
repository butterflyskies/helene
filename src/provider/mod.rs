mod types;

pub use types::{
    ChatMessage, CompletionRequest, CompletionResponse, ModelId, ResponseContent, Role, StopReason,
    ToolCall, ToolDefinition, Usage,
};

use std::future::Future;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("rate limited; retry after {retry_after_ms:?}ms")]
    RateLimited { retry_after_ms: Option<u64> },

    #[error("model not found: {0}")]
    ModelNotFound(ModelId),

    #[error("context length exceeded: {used} tokens exceeds {limit}")]
    ContextLength { used: u32, limit: u32 },

    #[error("provider unavailable: {0}")]
    Unavailable(String),

    #[error("request failed: {0}")]
    Request(String),
}

pub trait InferenceProvider: Send + Sync {
    fn complete(
        &self,
        request: &CompletionRequest,
    ) -> impl Future<Output = Result<CompletionResponse, ProviderError>> + Send;
}

#[cfg(test)]
mod tests;
