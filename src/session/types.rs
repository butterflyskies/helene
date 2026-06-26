use std::fmt;
use std::time::Instant;

use crate::provider::ModelId;
use crate::transport::TenantId;

/// Unique identifier for a session instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SessionId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SessionId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// Configuration for a single tenant's session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub tenant_id: TenantId,
    pub model: ModelId,
    pub max_tokens: u32,
    pub system_prompt: Option<String>,
    pub temperature: Option<f64>,
}

impl SessionConfig {
    pub fn new(tenant_id: TenantId, model: ModelId, max_tokens: u32) -> Self {
        Self {
            tenant_id,
            model,
            max_tokens,
            system_prompt: None,
            temperature: None,
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn with_temperature(mut self, temp: f64) -> Self {
        self.temperature = Some(temp);
        self
    }
}

/// Health status of a session.
#[derive(Debug, Clone)]
pub struct SessionHealth {
    pub session_id: SessionId,
    pub tenant_id: TenantId,
    pub connected: bool,
    pub messages_processed: u64,
    pub verification_failures: u64,
    pub last_message_at: Option<Instant>,
}

/// The result of processing an inbound message through the session pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessResult {
    Response(String),
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
}
