use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

use crate::context::Context;
use crate::provider::{CompletionResponse, InferenceProvider, ResponseContent, Role};
use crate::transport::TenantId;
use crate::verify::{Message, MessageVerifier, SignedMessage};

use super::{
    ProcessResult, SessionConfig, SessionError, SessionHealth, SessionHost, SessionId,
};

struct TenantSession {
    id: SessionId,
    config: SessionConfig,
    context: Context,
    messages_processed: u64,
    verification_failures: u64,
    last_message_at: Option<Instant>,
}

impl TenantSession {
    fn new(id: SessionId, config: SessionConfig) -> Self {
        let mut context = Context::new(config.model.clone(), config.max_tokens);
        if let Some(ref prompt) = config.system_prompt {
            context.set_system_prompt(prompt.clone());
        }
        if let Some(temp) = config.temperature {
            context.set_temperature(Some(temp));
        }
        Self {
            id,
            config,
            context,
            messages_processed: 0,
            verification_failures: 0,
            last_message_at: None,
        }
    }

    fn health(&self, connected: bool) -> SessionHealth {
        SessionHealth {
            session_id: self.id.clone(),
            tenant_id: self.config.tenant_id.clone(),
            connected,
            messages_processed: self.messages_processed,
            verification_failures: self.verification_failures,
            last_message_at: self.last_message_at,
        }
    }
}

/// In-memory session host backed by a map of tenant sessions.
///
/// Each tenant gets isolated state: its own conversation context,
/// message counters, and health tracking. The verifier and provider
/// are shared across tenants (the verifier key can vary per-tenant
/// in a future multi-key implementation).
pub struct InMemorySessionHost<V, P> {
    verifier: Arc<V>,
    provider: Arc<P>,
    sessions: Arc<RwLock<HashMap<TenantId, TenantSession>>>,
    connected: bool,
}

impl<V, P> InMemorySessionHost<V, P>
where
    V: MessageVerifier,
    P: InferenceProvider,
{
    pub fn new(verifier: V, provider: P) -> Self {
        Self {
            verifier: Arc::new(verifier),
            provider: Arc::new(provider),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            connected: true,
        }
    }

    /// Verify a signed message against the session's verifier.
    pub fn verify(&self, signed: &SignedMessage) -> Result<Message, SessionError> {
        self.verifier.verify(signed).map_err(SessionError::from)
    }

    /// Process a verified message: append to context, run inference, return response.
    async fn process_verified(
        &self,
        tenant_id: &TenantId,
        message: Message,
    ) -> Result<CompletionResponse, SessionError> {
        {
            let mut sessions = self.sessions.write().await;
            let session = sessions
                .get_mut(tenant_id)
                .ok_or_else(|| SessionError::TenantNotFound(tenant_id.clone()))?;

            session.context.push(message, Role::User);
            session.messages_processed += 1;
            session.last_message_at = Some(Instant::now());
        }

        let request = {
            let sessions = self.sessions.read().await;
            let session = sessions
                .get(tenant_id)
                .ok_or_else(|| SessionError::TenantNotFound(tenant_id.clone()))?;
            session.context.to_completion_request()
        };

        let response = self.provider.complete(&request).await?;

        // Append assistant response to context for multi-turn conversations.
        if let ResponseContent::Text(ref text) = response.content {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(tenant_id) {
                let assistant_msg = Message {
                    channel_id: crate::verify::ChannelId("session".into()),
                    message_id: crate::verify::MessageId(format!(
                        "resp-{}",
                        session.messages_processed
                    )),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    author: "assistant".into(),
                    content: text.clone(),
                };
                session.context.push(assistant_msg, Role::Assistant);
            }
        }

        Ok(response)
    }

    /// Record a verification failure for a tenant.
    pub async fn record_verification_failure(&self, tenant_id: &TenantId) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(tenant_id) {
            session.verification_failures += 1;
        }
    }

    /// Extract a process result from a completion response.
    pub fn extract_result(response: &CompletionResponse) -> ProcessResult {
        match &response.content {
            ResponseContent::ToolCalls(calls) => {
                if let Some(call) = calls.first() {
                    ProcessResult::ToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    }
                } else {
                    ProcessResult::Response(String::new())
                }
            }
            ResponseContent::Text(text) => ProcessResult::Response(text.clone()),
        }
    }
}

impl<V, P> SessionHost for InMemorySessionHost<V, P>
where
    V: MessageVerifier,
    P: InferenceProvider,
{
    fn process_message(
        &self,
        tenant_id: &TenantId,
        message: Message,
    ) -> impl std::future::Future<Output = Result<CompletionResponse, SessionError>> + Send {
        self.process_verified(tenant_id, message)
    }

    fn register_tenant(
        &self,
        config: SessionConfig,
    ) -> impl std::future::Future<Output = Result<SessionId, SessionError>> + Send {
        let sessions = self.sessions.clone();
        let tenant_id = config.tenant_id.clone();
        async move {
            let id = SessionId(format!("session-{}", tenant_id));
            let session = TenantSession::new(id.clone(), config);
            let mut map = sessions.write().await;
            map.insert(tenant_id, session);
            Ok(id)
        }
    }

    fn remove_tenant(
        &self,
        tenant_id: &TenantId,
    ) -> impl std::future::Future<Output = Result<(), SessionError>> + Send {
        let sessions = self.sessions.clone();
        let tenant_id = tenant_id.clone();
        async move {
            let mut map = sessions.write().await;
            map.remove(&tenant_id)
                .ok_or(SessionError::TenantNotFound(tenant_id))?;
            Ok(())
        }
    }

    fn health(
        &self,
        tenant_id: &TenantId,
    ) -> impl std::future::Future<Output = Result<SessionHealth, SessionError>> + Send {
        let sessions = self.sessions.clone();
        let tenant_id = tenant_id.clone();
        let connected = self.connected;
        async move {
            let map = sessions.read().await;
            let session = map
                .get(&tenant_id)
                .ok_or(SessionError::TenantNotFound(tenant_id))?;
            Ok(session.health(connected))
        }
    }

    fn tenants(&self) -> impl std::future::Future<Output = Vec<TenantId>> + Send {
        let sessions = self.sessions.clone();
        async move {
            let map = sessions.read().await;
            map.keys().cloned().collect()
        }
    }
}
