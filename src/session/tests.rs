use crate::provider::{
    CompletionRequest, CompletionResponse, InferenceProvider, ModelId, ProviderError,
    ResponseContent, Role, StopReason, ToolCall, Usage,
};
use crate::transport::TenantId;
use crate::verify::{ChannelId, HmacVerifier, Message, MessageId, MessageVerifier};

use super::*;

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};

/// Mock provider that echoes back the last user message.
struct EchoProvider {
    call_count: AtomicU64,
}

impl EchoProvider {
    fn new() -> Self {
        Self {
            call_count: AtomicU64::new(0),
        }
    }
}

impl InferenceProvider for EchoProvider {
    fn complete(
        &self,
        request: &CompletionRequest,
    ) -> impl Future<Output = Result<CompletionResponse, ProviderError>> + Send {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        let last_content = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| format!("echo: {}", m.content))
            .unwrap_or_else(|| "echo: <empty>".into());

        async move {
            Ok(CompletionResponse {
                content: ResponseContent::Text(last_content),
                model: ModelId::new("echo-1"),
                stop_reason: StopReason::EndTurn,
                usage: Usage::new(10, 5),
            })
        }
    }
}

/// Mock provider that returns tool calls.
struct ToolProvider;

impl InferenceProvider for ToolProvider {
    fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> impl Future<Output = Result<CompletionResponse, ProviderError>> + Send {
        async {
            Ok(CompletionResponse {
                content: ResponseContent::ToolCalls(vec![ToolCall {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    arguments: r#"{"city":"seattle"}"#.into(),
                }]),
                model: ModelId::new("tool-1"),
                stop_reason: StopReason::ToolUse,
                usage: Usage::new(10, 5),
            })
        }
    }
}

/// Mock provider that always fails.
struct FailProvider;

impl InferenceProvider for FailProvider {
    fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> impl Future<Output = Result<CompletionResponse, ProviderError>> + Send {
        async { Err(ProviderError::Unavailable("down for maintenance".into())) }
    }
}

fn test_message(content: &str) -> Message {
    Message {
        channel_id: ChannelId("test-channel".into()),
        message_id: MessageId("msg-1".into()),
        timestamp: 1000,
        author: "alice".into(),
        content: content.into(),
    }
}

fn test_config() -> SessionConfig {
    SessionConfig::new(TenantId("lain".into()), ModelId::new("test-model"), 4096)
}

fn test_host() -> InMemorySessionHost<HmacVerifier, EchoProvider> {
    InMemorySessionHost::new(
        HmacVerifier::new(b"test-key".to_vec()).unwrap(),
        EchoProvider::new(),
    )
}

#[tokio::test]
async fn register_and_list_tenants() {
    let host = test_host();
    assert!(host.tenants().await.is_empty());

    let id = host.register_tenant(test_config()).await.unwrap();
    assert_eq!(id.0, "session-lain");

    let tenants = host.tenants().await;
    assert_eq!(tenants.len(), 1);
    assert_eq!(tenants[0].0, "lain");
}

#[tokio::test]
async fn register_multiple_tenants() {
    let host = test_host();

    let cfg_a = SessionConfig::new(TenantId("ari".into()), ModelId::new("m"), 4096);
    let cfg_b = SessionConfig::new(TenantId("vesper".into()), ModelId::new("m"), 4096);

    host.register_tenant(cfg_a).await.unwrap();
    host.register_tenant(cfg_b).await.unwrap();

    let mut tenants = host.tenants().await;
    tenants.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(tenants.len(), 2);
    assert_eq!(tenants[0].0, "ari");
    assert_eq!(tenants[1].0, "vesper");
}

#[tokio::test]
async fn remove_tenant() {
    let host = test_host();
    host.register_tenant(test_config()).await.unwrap();
    assert_eq!(host.tenants().await.len(), 1);

    host.remove_tenant(&TenantId("lain".into())).await.unwrap();
    assert!(host.tenants().await.is_empty());
}

#[tokio::test]
async fn remove_nonexistent_tenant() {
    let host = test_host();
    let result = host.remove_tenant(&TenantId("ghost".into())).await;
    assert!(matches!(result, Err(SessionError::TenantNotFound(_))));
}

#[tokio::test]
async fn process_message_echoes() {
    let host = test_host();
    host.register_tenant(test_config()).await.unwrap();

    let msg = test_message("hello world");
    let response = host
        .process_message(&TenantId("lain".into()), msg)
        .await
        .unwrap();

    match &response.content {
        ResponseContent::Text(text) => assert_eq!(text, "echo: hello world"),
        _ => panic!("expected text response"),
    }
}

#[tokio::test]
async fn process_message_increments_counter() {
    let host = test_host();
    host.register_tenant(test_config()).await.unwrap();
    let tenant = TenantId("lain".into());

    host.process_message(&tenant, test_message("one"))
        .await
        .unwrap();
    host.process_message(&tenant, test_message("two"))
        .await
        .unwrap();

    let health = host.health(&tenant).await.unwrap();
    assert_eq!(health.messages_processed, 2);
}

#[tokio::test]
async fn process_message_updates_last_message_time() {
    let host = test_host();
    host.register_tenant(test_config()).await.unwrap();
    let tenant = TenantId("lain".into());

    let health_before = host.health(&tenant).await.unwrap();
    assert!(health_before.last_message_at_millis.is_none());

    host.process_message(&tenant, test_message("hi"))
        .await
        .unwrap();

    let health_after = host.health(&tenant).await.unwrap();
    assert!(health_after.last_message_at_millis.is_some());
}

#[tokio::test]
async fn process_message_unknown_tenant() {
    let host = test_host();
    let result = host
        .process_message(&TenantId("nobody".into()), test_message("hi"))
        .await;
    assert!(matches!(result, Err(SessionError::TenantNotFound(_))));
}

#[tokio::test]
async fn process_appends_to_context() {
    let host = test_host();
    host.register_tenant(test_config()).await.unwrap();
    let tenant = TenantId("lain".into());

    host.process_message(&tenant, test_message("first"))
        .await
        .unwrap();
    let response = host
        .process_message(&tenant, test_message("second"))
        .await
        .unwrap();

    match &response.content {
        ResponseContent::Text(text) => assert_eq!(text, "echo: second"),
        _ => panic!("expected text response"),
    }
    assert_eq!(host.health(&tenant).await.unwrap().messages_processed, 2);
}

#[tokio::test]
async fn health_reports_connected() {
    let host = test_host();
    host.register_tenant(test_config()).await.unwrap();

    let health = host.health(&TenantId("lain".into())).await.unwrap();
    assert!(health.connected);
    assert_eq!(health.messages_processed, 0);
    assert_eq!(health.verification_failures, 0);
}

#[tokio::test]
async fn health_reports_disconnected() {
    let host = test_host();
    host.register_tenant(test_config()).await.unwrap();
    host.set_connected(false);

    let health = host.health(&TenantId("lain".into())).await.unwrap();
    assert!(!health.connected);
}

#[tokio::test]
async fn health_unknown_tenant() {
    let host = test_host();
    let result = host.health(&TenantId("ghost".into())).await;
    assert!(matches!(result, Err(SessionError::TenantNotFound(_))));
}

#[tokio::test]
async fn verification_failure_tracking() {
    let host = test_host();
    host.register_tenant(test_config()).await.unwrap();
    let tenant = TenantId("lain".into());

    host.record_verification_failure(&tenant).await;
    host.record_verification_failure(&tenant).await;

    let health = host.health(&tenant).await.unwrap();
    assert_eq!(health.verification_failures, 2);
}

#[tokio::test]
async fn verify_valid_message() {
    let host = test_host();
    let msg = test_message("hello");
    let verifier = HmacVerifier::new(b"test-key".to_vec()).unwrap();
    let signed = verifier.sign(&msg);

    let verified = host.verify(&signed).unwrap();
    assert_eq!(verified.content, "hello");
}

#[tokio::test]
async fn verify_tampered_message() {
    let host = test_host();
    let msg = test_message("hello");
    let verifier = HmacVerifier::new(b"test-key".to_vec()).unwrap();
    let mut signed = verifier.sign(&msg);
    signed.message.content = "tampered".into();

    assert!(host.verify(&signed).is_err());
}

#[tokio::test]
async fn verify_wrong_key() {
    let host = test_host();
    let msg = test_message("hello");
    let wrong_verifier = HmacVerifier::new(b"wrong-key".to_vec()).unwrap();
    let signed = wrong_verifier.sign(&msg);

    assert!(host.verify(&signed).is_err());
}

#[tokio::test]
async fn extract_text_response() {
    let response = CompletionResponse {
        content: ResponseContent::Text("hello back".into()),
        model: ModelId::new("m"),
        stop_reason: StopReason::EndTurn,
        usage: Usage::new(1, 1),
    };

    let result = ProcessResult::from_response(&response);
    assert_eq!(result, ProcessResult::Response("hello back".into()));
}

#[tokio::test]
async fn extract_tool_call_response() {
    let response = CompletionResponse {
        content: ResponseContent::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: "search".into(),
            arguments: "{}".into(),
        }]),
        model: ModelId::new("m"),
        stop_reason: StopReason::ToolUse,
        usage: Usage::new(1, 1),
    };

    let result = ProcessResult::from_response(&response);
    assert!(matches!(result, ProcessResult::ToolCall { name, .. } if name == "search"));
}

#[tokio::test]
async fn system_prompt_in_config() {
    let host = test_host();
    let config = test_config().with_system_prompt("You are helpful.");
    host.register_tenant(config).await.unwrap();

    let response = host
        .process_message(&TenantId("lain".into()), test_message("hi"))
        .await
        .unwrap();

    match &response.content {
        ResponseContent::Text(text) => assert_eq!(text, "echo: hi"),
        _ => panic!("expected text response"),
    }
}

#[tokio::test]
async fn temperature_in_config() {
    let host = test_host();
    let config = test_config().with_temperature(0.7);
    host.register_tenant(config).await.unwrap();

    let response = host
        .process_message(&TenantId("lain".into()), test_message("hi"))
        .await
        .unwrap();

    assert!(matches!(response.content, ResponseContent::Text(_)));
}

#[tokio::test]
async fn provider_error_propagates() {
    let host = InMemorySessionHost::new(HmacVerifier::new(b"key".to_vec()).unwrap(), FailProvider);
    host.register_tenant(test_config()).await.unwrap();

    let result = host
        .process_message(&TenantId("lain".into()), test_message("hi"))
        .await;

    assert!(matches!(result, Err(SessionError::Provider(_))));
}

#[tokio::test]
async fn tool_call_response_handling() {
    let host = InMemorySessionHost::new(HmacVerifier::new(b"key".to_vec()).unwrap(), ToolProvider);
    host.register_tenant(test_config()).await.unwrap();

    let response = host
        .process_message(
            &TenantId("lain".into()),
            test_message("what's the weather?"),
        )
        .await
        .unwrap();

    match &response.content {
        ResponseContent::ToolCalls(calls) => assert_eq!(calls[0].name, "get_weather"),
        _ => panic!("expected tool call response"),
    }
}

#[tokio::test]
async fn multi_tenant_isolation() {
    let host = InMemorySessionHost::new(
        HmacVerifier::new(b"key".to_vec()).unwrap(),
        EchoProvider::new(),
    );

    let cfg_a = SessionConfig::new(TenantId("ari".into()), ModelId::new("m"), 4096)
        .with_system_prompt("You are Ari.");
    let cfg_b = SessionConfig::new(TenantId("lain".into()), ModelId::new("m"), 4096)
        .with_system_prompt("You are Lain.");

    host.register_tenant(cfg_a).await.unwrap();
    host.register_tenant(cfg_b).await.unwrap();

    host.process_message(&TenantId("ari".into()), test_message("hello from ari"))
        .await
        .unwrap();

    let health_ari = host.health(&TenantId("ari".into())).await.unwrap();
    let health_lain = host.health(&TenantId("lain".into())).await.unwrap();

    assert_eq!(health_ari.messages_processed, 1);
    assert_eq!(health_lain.messages_processed, 0);
}

#[tokio::test]
async fn duplicate_tenant_rejected() {
    let host = test_host();
    host.register_tenant(test_config()).await.unwrap();
    let result = host.register_tenant(test_config()).await;
    assert!(matches!(result, Err(SessionError::TenantAlreadyExists(_))));
}
