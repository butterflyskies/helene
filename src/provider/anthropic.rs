//! Anthropic Messages API inference provider.

use std::fmt;
use std::time::{Duration, SystemTime};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroize;

use super::{
    CompletionRequest, CompletionResponse, InferenceProvider, ModelId, ProviderError,
    ResponseContent, Role, StopReason, ToolCall, ToolDefinition, Usage,
};

const DEFAULT_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

// ---------------------------------------------------------------------------
// API key wrapper
// ---------------------------------------------------------------------------

/// An Anthropic API key, redacted in [`Debug`] and zeroized on [`Drop`].
pub struct ApiKey(String);

impl ApiKey {
    /// Create a new API key wrapper.
    ///
    /// Returns an error if the key is empty, following the same validation
    /// pattern as [`HmacVerifier::new`](crate::verify::HmacVerifier::new).
    pub fn new(key: impl Into<String>) -> Result<Self, ProviderError> {
        let key = key.into();
        if key.is_empty() {
            return Err(ProviderError::Auth("API key cannot be empty".into()));
        }
        Ok(Self(key))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey([REDACTED])")
    }
}

impl Drop for ApiKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Anthropic Messages API inference provider.
///
/// Sends requests to the Anthropic Messages API and maps responses back to
/// the provider-agnostic [`CompletionResponse`] type.
pub struct AnthropicProvider {
    client: Client,
    api_key: ApiKey,
    api_url: String,
}

impl AnthropicProvider {
    /// Create a new provider with the given API key.
    ///
    /// Uses sensible default timeouts (30 s connect, 600 s request).
    pub fn new(api_key: ApiKey) -> Result<Self, ProviderError> {
        Ok(Self {
            client: Client::builder()
                .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
                .timeout(DEFAULT_REQUEST_TIMEOUT)
                .build()
                .map_err(|e| ProviderError::Request(format!("failed to build HTTP client: {e}")))?,
            api_key,
            api_url: DEFAULT_API_URL.into(),
        })
    }

    /// Override the API endpoint (useful for testing against a mock server).
    #[must_use]
    pub fn with_api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = url.into();
        self
    }
}

impl fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("api_key", &self.api_key)
            .field("api_url", &self.api_url)
            .finish()
    }
}

impl InferenceProvider for AnthropicProvider {
    async fn complete(
        &self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        let api_req = build_api_request(request)?;

        let http_resp = self
            .client
            .post(&self.api_url)
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&api_req)
            .send()
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;

        if !http_resp.status().is_success() {
            return Err(map_error_response(http_resp, &request.model).await);
        }

        let api_resp: ApiResponse = http_resp
            .json()
            .await
            .map_err(|e| ProviderError::Request(format!("failed to parse response: {e}")))?;

        parse_api_response(api_resp)
    }
}

// ---------------------------------------------------------------------------
// Anthropic API wire types (serde)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ApiTool>>,
}

/// A message in the Anthropic messages array (request-side only).
#[derive(Debug, Clone, Serialize, PartialEq)]
struct ApiMessage {
    role: String,
    content: MessageContent,
}

/// Message content: either a plain string or an array of content blocks.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(untagged)]
enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

/// A typed content block for request serialization.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    /// Tool invocation block. Constructed when building assistant messages
    /// that carry tool calls (multi-turn tool-use conversations) and when
    /// deserializing API responses in tests.
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Serialize)]
struct ApiTool {
    name: String,
    description: String,
    input_schema: Value,
}

/// Anthropic API response. Content blocks are deserialized as raw [`Value`]s
/// so that unknown block types (e.g. `thinking`) don't cause parse failures.
#[derive(Debug, Deserialize)]
struct ApiResponse {
    model: String,
    content: Vec<Value>,
    stop_reason: Option<String>,
    usage: ApiUsage,
}

#[derive(Debug, Deserialize)]
struct ApiUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

/// Convert a provider-agnostic [`CompletionRequest`] into an Anthropic API
/// request body.
///
/// - [`Role::System`] messages are extracted into the `system` parameter.
/// - [`Role::Tool`] messages become `tool_result` content blocks inside a
///   `user`-role message.
/// - Consecutive same-role messages are merged (the API rejects them).
fn build_api_request(request: &CompletionRequest) -> Result<ApiRequest, ProviderError> {
    let mut system: Option<String> = None;
    let mut messages: Vec<ApiMessage> = Vec::new();

    for msg in &request.messages {
        match msg.role {
            Role::System => match &mut system {
                Some(s) => {
                    s.push('\n');
                    s.push_str(&msg.content);
                }
                None => system = Some(msg.content.clone()),
            },
            Role::User => {
                push_or_merge(
                    &mut messages,
                    "user",
                    MessageContent::Text(msg.content.clone()),
                );
            }
            Role::Assistant => {
                let content = match &msg.tool_calls {
                    Some(calls) if !calls.is_empty() => {
                        let mut blocks = Vec::new();
                        if !msg.content.is_empty() {
                            blocks.push(ContentBlock::Text {
                                text: msg.content.clone(),
                            });
                        }
                        for tc in calls {
                            let input: Value =
                                serde_json::from_str(&tc.arguments).unwrap_or_default();
                            blocks.push(ContentBlock::ToolUse {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                input,
                            });
                        }
                        MessageContent::Blocks(blocks)
                    }
                    _ => MessageContent::Text(msg.content.clone()),
                };
                push_or_merge(&mut messages, "assistant", content);
            }
            Role::Tool => {
                let tool_use_id = msg
                    .tool_call_id
                    .as_deref()
                    .ok_or_else(|| {
                        ProviderError::Request(
                            "tool message missing tool_call_id \
                             (required for Anthropic API)"
                                .into(),
                        )
                    })?
                    .to_owned();
                let block = ContentBlock::ToolResult {
                    tool_use_id,
                    content: msg.content.clone(),
                };
                push_or_merge(&mut messages, "user", MessageContent::Blocks(vec![block]));
            }
        }
    }

    if messages.is_empty() {
        return Err(ProviderError::Request(
            "no messages after extracting system prompt".into(),
        ));
    }

    let tools = request
        .tools
        .as_ref()
        .map(|ts| {
            ts.iter()
                .map(map_tool_definition)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;

    Ok(ApiRequest {
        model: request.model.0.clone(),
        max_tokens: request.max_tokens,
        messages,
        system,
        temperature: request.temperature,
        tools,
    })
}

/// Push a message, merging into the previous one when both share the same
/// role. The Anthropic API rejects consecutive same-role messages, so this
/// collapses e.g. multiple `tool_result` blocks into a single `user` message.
fn push_or_merge(messages: &mut Vec<ApiMessage>, role: &str, content: MessageContent) {
    if let Some(last) = messages.last_mut()
        && last.role == role
    {
        let prev = std::mem::replace(&mut last.content, MessageContent::Blocks(Vec::new()));
        let mut blocks = into_blocks(prev);
        blocks.extend(into_blocks(content));
        last.content = MessageContent::Blocks(blocks);
        return;
    }
    messages.push(ApiMessage {
        role: role.into(),
        content,
    });
}

/// Convert a [`MessageContent`] into a vector of blocks, consuming it.
fn into_blocks(content: MessageContent) -> Vec<ContentBlock> {
    match content {
        MessageContent::Text(t) => vec![ContentBlock::Text { text: t }],
        MessageContent::Blocks(b) => b,
    }
}

fn map_tool_definition(tool: &ToolDefinition) -> Result<ApiTool, ProviderError> {
    let input_schema: Value = serde_json::from_str(&tool.input_schema).map_err(|e| {
        ProviderError::Request(format!(
            "invalid JSON in input_schema for tool '{}': {e}",
            tool.name
        ))
    })?;

    Ok(ApiTool {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema,
    })
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

/// Convert an Anthropic API response into a provider-agnostic
/// [`CompletionResponse`].
///
/// Content blocks are [`Value`]s — we match on the `type` field and silently
/// skip unknown block types for forward compatibility. When both `text` and
/// `tool_use` blocks are present, tool calls take precedence (the model
/// expects tool results before continuing).
fn parse_api_response(resp: ApiResponse) -> Result<CompletionResponse, ProviderError> {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    for block in &resp.content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    text_parts.push(text.to_owned());
                }
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ProviderError::Request("tool_use block missing required 'id' field".into())
                    })?
                    .to_owned();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ProviderError::Request(
                            "tool_use block missing required 'name' field".into(),
                        )
                    })?
                    .to_owned();
                let input = block
                    .get("input")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()));
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments: input.to_string(),
                });
            }
            // Unknown block types silently skipped for forward compatibility.
            _ => {}
        }
    }

    let content = if tool_calls.is_empty() {
        ResponseContent::Text(text_parts.join(""))
    } else {
        let text = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        };
        ResponseContent::ToolCalls {
            calls: tool_calls,
            text,
        }
    };

    let stop_reason = match resp.stop_reason.as_deref() {
        Some("end_turn") => StopReason::EndTurn,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("tool_use") => StopReason::ToolUse,
        Some("stop_sequence") => StopReason::StopSequence,
        Some(other) => {
            // Unlike unknown content blocks (which are additive and safely
            // skippable), stop_reason drives control flow — the caller needs
            // to know whether to send tool results, truncate, or stop.
            // Hard-failing surfaces the gap immediately rather than silently
            // misrouting the conversation.
            return Err(ProviderError::Request(format!(
                "unknown stop_reason: {other}"
            )));
        }
        None => {
            return Err(ProviderError::Request(
                "response missing stop_reason".into(),
            ));
        }
    };

    Ok(CompletionResponse {
        content,
        model: ModelId::new(resp.model),
        usage: Usage::new(resp.usage.input_tokens, resp.usage.output_tokens),
        stop_reason,
    })
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/// Map an HTTP error response to a [`ProviderError`].
async fn map_error_response(response: reqwest::Response, model: &ModelId) -> ProviderError {
    let status = response.status().as_u16();
    let retry_after_ms = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_retry_after(v));
    let body = response.text().await.unwrap_or_default();

    map_error(status, retry_after_ms, &body, model)
}

/// Parse a `Retry-After` header value as either delay-seconds (integer or
/// fractional) or an HTTP-date (RFC 7231 §7.1.1.1). Returns milliseconds.
fn parse_retry_after(value: &str) -> Option<u64> {
    // Try delay-seconds first (most common from Anthropic).
    if let Ok(secs) = value.parse::<f64>() {
        return Some((secs * 1000.0) as u64);
    }
    // Try HTTP-date (e.g. "Thu, 01 Dec 1994 16:00:00 GMT").
    let target = httpdate::parse_http_date(value).ok()?;
    let now = SystemTime::now();
    let duration = target.duration_since(now).ok()?;
    Some(duration.as_millis() as u64)
}

/// Pure mapping from HTTP status + body to [`ProviderError`].
/// Separated from [`map_error_response`] for testability.
fn map_error(
    status: u16,
    retry_after_ms: Option<u64>,
    body: &str,
    model: &ModelId,
) -> ProviderError {
    match status {
        401 => {
            ProviderError::Auth(parse_error_message(body).unwrap_or_else(|| "unauthorized".into()))
        }
        429 => ProviderError::RateLimited { retry_after_ms },
        404 => ProviderError::ModelNotFound(model.clone()),
        500 | 502 | 503 | 504 => {
            let msg = parse_error_message(body).unwrap_or_else(|| format!("HTTP {status}: {body}"));
            ProviderError::Unavailable(msg)
        }
        529 => ProviderError::Unavailable("API overloaded".into()),
        _ => {
            let msg = parse_error_message(body).unwrap_or_else(|| format!("HTTP {status}: {body}"));
            ProviderError::Request(msg)
        }
    }
}

/// Try to extract a human-readable error message from an Anthropic error
/// response body.
fn parse_error_message(body: &str) -> Option<String> {
    let parsed: ApiErrorBody = serde_json::from_str(body).ok()?;
    Some(format!(
        "{}: {}",
        parsed.error.error_type, parsed.error.message
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ChatMessage;
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;

    // -- Helpers --

    fn simple_request(messages: Vec<ChatMessage>) -> CompletionRequest {
        CompletionRequest {
            model: ModelId::new("claude-sonnet-4-20250514"),
            messages,
            max_tokens: 1024,
            temperature: None,
            tools: None,
        }
    }

    /// Build an [`ApiResponse`] from typed [`ContentBlock`]s, converting them
    /// to [`Value`]s via serde (matches how the real API response arrives).
    fn api_response(content: Vec<ContentBlock>, stop_reason: &str) -> ApiResponse {
        let content_values = content
            .into_iter()
            .map(|b| serde_json::to_value(b).unwrap())
            .collect();
        ApiResponse {
            model: "claude-sonnet-4-20250514".into(),
            content: content_values,
            stop_reason: Some(stop_reason.into()),
            usage: ApiUsage {
                input_tokens: 100,
                output_tokens: 50,
            },
        }
    }

    fn test_model() -> ModelId {
        ModelId::new("test-model")
    }

    // -- ApiKey --

    #[test]
    fn api_key_debug_redacted() {
        let key = ApiKey::new("sk-ant-super-secret-key-12345").unwrap();
        let debug = format!("{key:?}");
        assert_eq!(debug, "ApiKey([REDACTED])");
        assert!(!debug.contains("sk-ant"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn api_key_empty_rejected() {
        let err = ApiKey::new("").unwrap_err();
        assert!(err.to_string().contains("empty"), "error: {err}");
    }

    // -- Provider Debug --

    #[test]
    fn provider_debug_redacted() {
        let provider = AnthropicProvider::new(ApiKey::new("sk-ant-secret").unwrap()).unwrap();
        let debug = format!("{provider:?}");
        assert!(debug.contains("ApiKey([REDACTED])"));
        assert!(!debug.contains("sk-ant-secret"));
        assert!(debug.contains("api.anthropic.com"));
    }

    #[test]
    fn with_api_url_overrides_default() {
        let provider = AnthropicProvider::new(ApiKey::new("key").unwrap())
            .unwrap()
            .with_api_url("http://localhost:8080");
        let debug = format!("{provider:?}");
        assert!(debug.contains("localhost:8080"));
    }

    // -- Request building: system messages --

    #[test]
    fn system_message_extracted_to_parameter() {
        let req = simple_request(vec![
            ChatMessage::system("You are helpful."),
            ChatMessage::user("Hello"),
        ]);
        let api_req = build_api_request(&req).unwrap();

        assert_eq!(api_req.system, Some("You are helpful.".into()));
        assert_eq!(api_req.messages.len(), 1);
        assert_eq!(api_req.messages[0].role, "user");
    }

    #[test]
    fn multiple_system_messages_concatenated() {
        let req = simple_request(vec![
            ChatMessage::system("You are helpful."),
            ChatMessage::system("Be concise."),
            ChatMessage::user("Hello"),
        ]);
        let api_req = build_api_request(&req).unwrap();

        assert_eq!(api_req.system, Some("You are helpful.\nBe concise.".into()));
    }

    #[test]
    fn no_system_message_yields_none() {
        let req = simple_request(vec![ChatMessage::user("Hello")]);
        let api_req = build_api_request(&req).unwrap();

        assert!(api_req.system.is_none());
    }

    // -- Request building: basic messages --

    #[test]
    fn user_and_assistant_messages_mapped() {
        let req = simple_request(vec![
            ChatMessage::user("Hello"),
            ChatMessage::assistant("Hi there!"),
            ChatMessage::user("How are you?"),
        ]);
        let api_req = build_api_request(&req).unwrap();

        assert_eq!(api_req.messages.len(), 3);
        assert_eq!(api_req.messages[0].role, "user");
        assert_eq!(
            api_req.messages[0].content,
            MessageContent::Text("Hello".into())
        );
        assert_eq!(api_req.messages[1].role, "assistant");
        assert_eq!(
            api_req.messages[1].content,
            MessageContent::Text("Hi there!".into())
        );
        assert_eq!(api_req.messages[2].role, "user");
    }

    // -- Request building: tool messages --

    #[test]
    fn tool_result_mapped_to_content_block() {
        let req = simple_request(vec![
            ChatMessage::user("What is the weather?"),
            ChatMessage::assistant("Let me check."),
            ChatMessage::tool_result("toolu_01abc", r#"{"temp": 72}"#),
        ]);
        let api_req = build_api_request(&req).unwrap();

        assert_eq!(api_req.messages.len(), 3);
        assert_eq!(api_req.messages[2].role, "user");

        let blocks = match &api_req.messages[2].content {
            MessageContent::Blocks(b) => b,
            other => panic!("expected Blocks, got {other:?}"),
        };
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
            } => {
                assert_eq!(tool_use_id, "toolu_01abc");
                assert_eq!(content, r#"{"temp": 72}"#);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn tool_message_without_id_errors() {
        let req = simple_request(vec![
            ChatMessage::user("Hello"),
            ChatMessage::assistant("Using tool"),
            ChatMessage::tool("result without id"),
        ]);
        let err = build_api_request(&req).unwrap_err();
        assert!(
            err.to_string().contains("tool_call_id"),
            "error should mention tool_call_id: {err}"
        );
    }

    #[test]
    fn consecutive_tool_results_merged_into_one_message() {
        let req = simple_request(vec![
            ChatMessage::user("Do two things"),
            ChatMessage::assistant("OK"),
            ChatMessage::tool_result("toolu_01", "result 1"),
            ChatMessage::tool_result("toolu_02", "result 2"),
        ]);
        let api_req = build_api_request(&req).unwrap();

        // The two tool results should merge into a single user message.
        assert_eq!(api_req.messages.len(), 3);
        assert_eq!(api_req.messages[2].role, "user");

        let blocks = match &api_req.messages[2].content {
            MessageContent::Blocks(b) => b,
            other => panic!("expected Blocks, got {other:?}"),
        };
        assert_eq!(blocks.len(), 2);
        match (&blocks[0], &blocks[1]) {
            (
                ContentBlock::ToolResult {
                    tool_use_id: id0, ..
                },
                ContentBlock::ToolResult {
                    tool_use_id: id1, ..
                },
            ) => {
                assert_eq!(id0, "toolu_01");
                assert_eq!(id1, "toolu_02");
            }
            other => panic!("expected two ToolResult blocks, got {other:?}"),
        }
    }

    // -- Request building: assistant with tool calls (roundtrip) --

    #[test]
    fn assistant_with_tool_calls_emits_tool_use_blocks() {
        let tool_calls = vec![ToolCall {
            id: "toolu_01abc".into(),
            name: "get_weather".into(),
            arguments: r#"{"city":"Tokyo"}"#.into(),
        }];
        let req = simple_request(vec![
            ChatMessage::user("What's the weather?"),
            ChatMessage::assistant_with_tool_calls("Let me check.", tool_calls),
            ChatMessage::tool_result("toolu_01abc", r#"{"temp": 72}"#),
        ]);
        let api_req = build_api_request(&req).unwrap();
        let json = serde_json::to_value(&api_req).unwrap();

        // Assistant message should have text + tool_use blocks.
        let assistant_msg = &json["messages"][1];
        assert_eq!(assistant_msg["role"], "assistant");
        let blocks = assistant_msg["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "Let me check.");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["id"], "toolu_01abc");
        assert_eq!(blocks[1]["name"], "get_weather");
        assert_eq!(blocks[1]["input"]["city"], "Tokyo");
    }

    #[test]
    fn assistant_with_tool_calls_empty_text_omits_text_block() {
        let tool_calls = vec![ToolCall {
            id: "toolu_01".into(),
            name: "search".into(),
            arguments: r#"{"q":"rust"}"#.into(),
        }];
        let req = simple_request(vec![
            ChatMessage::user("Search"),
            ChatMessage::assistant_with_tool_calls("", tool_calls),
            ChatMessage::tool_result("toolu_01", "found it"),
        ]);
        let api_req = build_api_request(&req).unwrap();
        let json = serde_json::to_value(&api_req).unwrap();

        let blocks = json["messages"][1]["content"].as_array().unwrap();
        assert_eq!(
            blocks.len(),
            1,
            "empty text should not produce a text block"
        );
        assert_eq!(blocks[0]["type"], "tool_use");
    }

    // -- Request building: tool definitions --

    #[test]
    fn tool_definitions_serialized() {
        let req = CompletionRequest {
            model: ModelId::new("claude-sonnet-4-20250514"),
            messages: vec![ChatMessage::user("Hello")],
            max_tokens: 1024,
            temperature: None,
            tools: Some(vec![ToolDefinition {
                name: "get_weather".into(),
                description: "Get the weather".into(),
                input_schema: r#"{"type":"object","properties":{"city":{"type":"string"}}}"#.into(),
            }]),
        };
        let api_req = build_api_request(&req).unwrap();

        let tools = api_req.tools.as_ref().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "get_weather");
        assert_eq!(tools[0].description, "Get the weather");
        assert!(tools[0].input_schema.is_object());
    }

    #[test]
    fn invalid_tool_schema_rejected() {
        let req = CompletionRequest {
            model: ModelId::new("test"),
            messages: vec![ChatMessage::user("hi")],
            max_tokens: 1024,
            temperature: None,
            tools: Some(vec![ToolDefinition {
                name: "bad_tool".into(),
                description: "broken".into(),
                input_schema: "not valid json {{{".into(),
            }]),
        };
        let err = build_api_request(&req).unwrap_err();
        assert!(err.to_string().contains("bad_tool"));
    }

    // -- Request building: edge cases --

    #[test]
    fn only_system_messages_rejected() {
        let req = simple_request(vec![ChatMessage::system("system only")]);
        let err = build_api_request(&req).unwrap_err();
        assert!(err.to_string().contains("no messages"));
    }

    #[test]
    fn consecutive_user_messages_merged() {
        let req = simple_request(vec![
            ChatMessage::user("first"),
            ChatMessage::user("second"),
        ]);
        let api_req = build_api_request(&req).unwrap();

        assert_eq!(api_req.messages.len(), 1);
        assert_eq!(api_req.messages[0].role, "user");

        let blocks = match &api_req.messages[0].content {
            MessageContent::Blocks(b) => b,
            other => panic!("expected merged Blocks, got {other:?}"),
        };
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "first".into()
            }
        );
        assert_eq!(
            blocks[1],
            ContentBlock::Text {
                text: "second".into()
            }
        );
    }

    #[test]
    fn request_model_and_max_tokens_mapped() {
        let req = CompletionRequest {
            model: ModelId::new("claude-opus-4-20250514"),
            messages: vec![ChatMessage::user("hi")],
            max_tokens: 4096,
            temperature: Some(0.7),
            tools: None,
        };
        let api_req = build_api_request(&req).unwrap();

        assert_eq!(api_req.model, "claude-opus-4-20250514");
        assert_eq!(api_req.max_tokens, 4096);
        assert_eq!(api_req.temperature, Some(0.7));
    }

    // -- Response parsing --

    #[test]
    fn text_response_parsed() {
        let resp = api_response(
            vec![ContentBlock::Text {
                text: "Hello world".into(),
            }],
            "end_turn",
        );
        let result = parse_api_response(resp).unwrap();

        assert_eq!(result.content, ResponseContent::Text("Hello world".into()));
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        assert_eq!(result.model.0, "claude-sonnet-4-20250514");
        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 50);
    }

    #[test]
    fn multi_text_blocks_concatenated() {
        let resp = api_response(
            vec![
                ContentBlock::Text {
                    text: "Hello ".into(),
                },
                ContentBlock::Text {
                    text: "world".into(),
                },
            ],
            "end_turn",
        );
        let result = parse_api_response(resp).unwrap();
        assert_eq!(result.content, ResponseContent::Text("Hello world".into()));
    }

    #[test]
    fn tool_use_response_parsed() {
        let resp = api_response(
            vec![ContentBlock::ToolUse {
                id: "toolu_01abc".into(),
                name: "get_weather".into(),
                input: serde_json::json!({"city": "Tokyo"}),
            }],
            "tool_use",
        );
        let result = parse_api_response(resp).unwrap();

        match &result.content {
            ResponseContent::ToolCalls { calls, text } => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "toolu_01abc");
                assert_eq!(calls[0].name, "get_weather");
                assert_eq!(calls[0].arguments, r#"{"city":"Tokyo"}"#);
                assert!(text.is_none(), "tool-only response should have no text");
            }
            other => panic!("expected ToolCalls, got {other:?}"),
        }
        assert_eq!(result.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn multiple_tool_calls_parsed() {
        let resp = api_response(
            vec![
                ContentBlock::ToolUse {
                    id: "toolu_01".into(),
                    name: "search".into(),
                    input: serde_json::json!({"q": "rust"}),
                },
                ContentBlock::ToolUse {
                    id: "toolu_02".into(),
                    name: "fetch".into(),
                    input: serde_json::json!({"url": "https://example.com"}),
                },
            ],
            "tool_use",
        );
        let result = parse_api_response(resp).unwrap();

        match &result.content {
            ResponseContent::ToolCalls { calls, .. } => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].id, "toolu_01");
                assert_eq!(calls[0].name, "search");
                assert_eq!(calls[0].arguments, r#"{"q":"rust"}"#);
                assert_eq!(calls[1].id, "toolu_02");
                assert_eq!(calls[1].name, "fetch");
            }
            other => panic!("expected ToolCalls, got {other:?}"),
        }
    }

    #[test]
    fn mixed_text_and_tool_use_preserves_both() {
        let resp = api_response(
            vec![
                ContentBlock::Text {
                    text: "Let me look that up.".into(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_01".into(),
                    name: "search".into(),
                    input: serde_json::json!({"q": "weather"}),
                },
            ],
            "tool_use",
        );
        let result = parse_api_response(resp).unwrap();

        match &result.content {
            ResponseContent::ToolCalls { calls, text } => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "toolu_01");
                assert_eq!(calls[0].name, "search");
                assert_eq!(text.as_deref(), Some("Let me look that up."));
            }
            other => panic!("expected ToolCalls, got {other:?}"),
        }
    }

    #[test]
    fn empty_content_yields_empty_text() {
        let resp = api_response(vec![], "end_turn");
        let result = parse_api_response(resp).unwrap();
        assert_eq!(result.content, ResponseContent::Text(String::new()));
    }

    // -- Response parsing: malformed tool_use --

    #[test]
    fn tool_use_block_missing_id_errors() {
        let resp = ApiResponse {
            model: "test".into(),
            content: vec![serde_json::json!({
                "type": "tool_use",
                "name": "search",
                "input": {"q": "rust"}
            })],
            stop_reason: Some("tool_use".into()),
            usage: ApiUsage {
                input_tokens: 1,
                output_tokens: 1,
            },
        };
        let err = parse_api_response(resp).unwrap_err();
        assert!(err.to_string().contains("id"), "should mention 'id': {err}");
    }

    #[test]
    fn tool_use_block_missing_name_errors() {
        let resp = ApiResponse {
            model: "test".into(),
            content: vec![serde_json::json!({
                "type": "tool_use",
                "id": "toolu_01",
                "input": {}
            })],
            stop_reason: Some("tool_use".into()),
            usage: ApiUsage {
                input_tokens: 1,
                output_tokens: 1,
            },
        };
        let err = parse_api_response(resp).unwrap_err();
        assert!(
            err.to_string().contains("name"),
            "should mention 'name': {err}"
        );
    }

    // -- Response parsing: stop reasons --

    #[test]
    fn all_stop_reasons_mapped() {
        for (wire, expected) in [
            ("end_turn", StopReason::EndTurn),
            ("max_tokens", StopReason::MaxTokens),
            ("tool_use", StopReason::ToolUse),
            ("stop_sequence", StopReason::StopSequence),
        ] {
            let resp = api_response(vec![ContentBlock::Text { text: "x".into() }], wire);
            let result = parse_api_response(resp).unwrap();
            assert_eq!(
                result.stop_reason, expected,
                "stop_reason '{wire}' mismatch"
            );
        }
    }

    #[test]
    fn unknown_stop_reason_rejected() {
        let resp = api_response(vec![ContentBlock::Text { text: "x".into() }], "yolo");
        let err = parse_api_response(resp).unwrap_err();
        assert!(err.to_string().contains("yolo"));
    }

    #[test]
    fn missing_stop_reason_rejected() {
        let resp = ApiResponse {
            model: "test".into(),
            content: vec![serde_json::json!({"type": "text", "text": "x"})],
            stop_reason: None,
            usage: ApiUsage {
                input_tokens: 1,
                output_tokens: 1,
            },
        };
        let err = parse_api_response(resp).unwrap_err();
        assert!(err.to_string().contains("stop_reason"));
    }

    // -- Error mapping --

    #[test]
    fn error_401_maps_to_auth() {
        let body = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid api key"}}"#;
        match map_error(401, None, body, &test_model()) {
            ProviderError::Auth(msg) => {
                assert!(msg.contains("authentication_error"));
                assert!(msg.contains("invalid api key"));
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn error_401_unparseable_body() {
        match map_error(401, None, "not json", &test_model()) {
            ProviderError::Auth(msg) => assert_eq!(msg, "unauthorized"),
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn error_429_maps_to_rate_limited() {
        match map_error(429, None, "", &test_model()) {
            ProviderError::RateLimited { retry_after_ms } => {
                assert!(retry_after_ms.is_none());
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn error_429_with_retry_after() {
        match map_error(429, Some(30_000), "", &test_model()) {
            ProviderError::RateLimited { retry_after_ms } => {
                assert_eq!(retry_after_ms, Some(30_000));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    // -- Retry-After parsing --

    #[test]
    fn parse_retry_after_seconds_integer() {
        assert_eq!(parse_retry_after("30"), Some(30_000));
    }

    #[test]
    fn parse_retry_after_seconds_fractional() {
        assert_eq!(parse_retry_after("1.5"), Some(1_500));
    }

    #[test]
    fn parse_retry_after_http_date() {
        // Use a date far in the future so it's always after SystemTime::now().
        let result = parse_retry_after("Sun, 01 Jan 2090 00:00:00 GMT");
        assert!(result.is_some(), "should parse HTTP-date");
        assert!(result.unwrap() > 0, "should be a positive duration");
    }

    #[test]
    fn parse_retry_after_past_date_returns_none() {
        // A date in the past yields None (duration_since would fail).
        assert!(parse_retry_after("Mon, 01 Jan 1990 00:00:00 GMT").is_none());
    }

    #[test]
    fn parse_retry_after_garbage_returns_none() {
        assert!(parse_retry_after("not-a-date-or-number").is_none());
    }

    #[test]
    fn error_404_maps_to_model_not_found() {
        let model = ModelId::new("claude-nonexistent-v1");
        match map_error(404, None, "", &model) {
            ProviderError::ModelNotFound(id) => {
                assert_eq!(id.0, "claude-nonexistent-v1");
            }
            other => panic!("expected ModelNotFound, got {other:?}"),
        }
    }

    #[test]
    fn error_404_unparseable_body_preserves_model() {
        let model = ModelId::new("claude-unknown");
        match map_error(404, None, "garbage", &model) {
            ProviderError::ModelNotFound(id) => {
                assert_eq!(id.0, "claude-unknown");
            }
            other => panic!("expected ModelNotFound, got {other:?}"),
        }
    }

    #[test]
    fn error_500_maps_to_unavailable() {
        let body = r#"{"type":"error","error":{"type":"api_error","message":"internal error"}}"#;
        match map_error(500, None, body, &test_model()) {
            ProviderError::Unavailable(msg) => {
                assert!(msg.contains("internal error"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn error_500_unparseable_body_includes_status() {
        match map_error(500, None, "kaboom", &test_model()) {
            ProviderError::Unavailable(msg) => {
                assert!(msg.contains("500"));
                assert!(msg.contains("kaboom"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn error_503_maps_to_unavailable() {
        match map_error(503, None, "", &test_model()) {
            ProviderError::Unavailable(_) => {}
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn error_504_maps_to_unavailable() {
        match map_error(504, None, "", &test_model()) {
            ProviderError::Unavailable(_) => {}
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn error_529_maps_to_unavailable() {
        match map_error(529, None, "", &test_model()) {
            ProviderError::Unavailable(msg) => {
                assert!(msg.contains("overloaded"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_message_valid() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens too large"}}"#;
        let msg = parse_error_message(body);
        assert_eq!(
            msg,
            Some("invalid_request_error: max_tokens too large".into())
        );
    }

    #[test]
    fn parse_error_message_invalid_json() {
        assert!(parse_error_message("not json").is_none());
    }

    #[test]
    fn parse_error_message_wrong_shape() {
        assert!(parse_error_message(r#"{"foo": "bar"}"#).is_none());
    }

    // -- Serialization roundtrips --

    #[test]
    fn api_request_serializes_correctly() {
        let req = simple_request(vec![
            ChatMessage::system("Be helpful"),
            ChatMessage::user("Hello"),
        ]);
        let api_req = build_api_request(&req).unwrap();
        let json = serde_json::to_value(&api_req).unwrap();

        assert_eq!(json["model"], "claude-sonnet-4-20250514");
        assert_eq!(json["max_tokens"], 1024);
        assert_eq!(json["system"], "Be helpful");
        assert_eq!(json["messages"].as_array().unwrap().len(), 1);
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][0]["content"], "Hello");
        // temperature and tools should be absent (skip_serializing_if)
        assert!(json.get("temperature").is_none());
        assert!(json.get("tools").is_none());
    }

    #[test]
    fn tool_result_serializes_correctly() {
        let req = simple_request(vec![
            ChatMessage::user("query"),
            ChatMessage::assistant("using tool"),
            ChatMessage::tool_result("toolu_01xyz", "42"),
        ]);
        let api_req = build_api_request(&req).unwrap();
        let json = serde_json::to_value(&api_req).unwrap();

        let tool_msg = &json["messages"][2];
        assert_eq!(tool_msg["role"], "user");
        let blocks = tool_msg["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[0]["tool_use_id"], "toolu_01xyz");
        assert_eq!(blocks[0]["content"], "42");
    }

    #[test]
    fn api_response_deserializes() {
        let json = serde_json::json!({
            "id": "msg_01abc",
            "type": "message",
            "model": "claude-sonnet-4-20250514",
            "content": [
                {"type": "text", "text": "Hello!"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 25,
                "output_tokens": 10
            }
        });

        let resp: ApiResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.model, "claude-sonnet-4-20250514");
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.content[0]["type"], "text");
        assert_eq!(resp.content[0]["text"], "Hello!");
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(resp.usage.input_tokens, 25);
    }

    #[test]
    fn api_response_with_tool_use_deserializes() {
        let json = serde_json::json!({
            "id": "msg_02def",
            "type": "message",
            "model": "claude-sonnet-4-20250514",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_01abc",
                    "name": "get_weather",
                    "input": {"city": "Tokyo"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 50, "output_tokens": 30}
        });

        let resp: ApiResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.content[0]["type"], "tool_use");
        assert_eq!(resp.content[0]["id"], "toolu_01abc");
        assert_eq!(resp.content[0]["name"], "get_weather");
        assert_eq!(resp.content[0]["input"]["city"], "Tokyo");
    }

    #[test]
    fn unknown_content_block_type_handled() {
        let json = serde_json::json!({
            "id": "msg_03",
            "type": "message",
            "model": "test",
            "content": [
                {"type": "thinking", "thinking": "hmm"},
                {"type": "text", "text": "result"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        });

        // Unknown block types are silently skipped.
        let resp: ApiResponse = serde_json::from_value(json).unwrap();
        let result = parse_api_response(resp).unwrap();
        assert_eq!(result.content, ResponseContent::Text("result".into()));
    }

    // -- Proptests --

    proptest! {
        #[test]
        fn system_never_in_api_messages(
            system in "\\PC{0,100}",
            user in "\\PC{1,100}",
        ) {
            let req = simple_request(vec![
                ChatMessage::system(system.clone()),
                ChatMessage::user(user),
            ]);
            let api_req = build_api_request(&req).unwrap();
            for msg in &api_req.messages {
                prop_assert_ne!(&msg.role, "system");
            }
            prop_assert_eq!(api_req.system.as_deref(), Some(system.as_str()));
        }

        #[test]
        fn api_key_debug_always_redacted(key in "\\PC{1,100}") {
            let api_key = ApiKey::new(key).unwrap();
            let debug = format!("{api_key:?}");
            prop_assert_eq!(debug, "ApiKey([REDACTED])");
        }

        #[test]
        fn build_preserves_message_ordering(
            user1 in "\\PC{1,50}",
            assistant1 in "\\PC{1,50}",
            user2 in "\\PC{1,50}",
        ) {
            let req = simple_request(vec![
                ChatMessage::user(user1.clone()),
                ChatMessage::assistant(assistant1.clone()),
                ChatMessage::user(user2.clone()),
            ]);
            let api_req = build_api_request(&req).unwrap();

            prop_assert_eq!(api_req.messages.len(), 3);
            prop_assert_eq!(&api_req.messages[0].role, "user");
            prop_assert_eq!(&api_req.messages[1].role, "assistant");
            prop_assert_eq!(&api_req.messages[2].role, "user");
            prop_assert_eq!(
                &api_req.messages[0].content,
                &MessageContent::Text(user1)
            );
            prop_assert_eq!(
                &api_req.messages[1].content,
                &MessageContent::Text(assistant1)
            );
            prop_assert_eq!(
                &api_req.messages[2].content,
                &MessageContent::Text(user2)
            );
        }

        #[test]
        fn tool_result_id_preserved(
            id in "[a-zA-Z0-9_]{1,30}",
            content in "\\PC{0,100}",
        ) {
            let req = simple_request(vec![
                ChatMessage::user("query"),
                ChatMessage::assistant("using tool"),
                ChatMessage::tool_result(id.clone(), content.clone()),
            ]);
            let api_req = build_api_request(&req).unwrap();

            let last = api_req.messages.last().unwrap();
            prop_assert_eq!(&last.role, "user");
            match &last.content {
                MessageContent::Blocks(blocks) => {
                    prop_assert_eq!(blocks.len(), 1);
                    match &blocks[0] {
                        ContentBlock::ToolResult { tool_use_id, content: c } => {
                            prop_assert_eq!(tool_use_id, &id);
                            prop_assert_eq!(c, &content);
                        }
                        other => prop_assert!(false, "expected ToolResult, got {:?}", other),
                    }
                }
                other => prop_assert!(false, "expected Blocks, got {:?}", other),
            }
        }

        #[test]
        fn valid_response_always_parses(
            text in "\\PC{0,200}",
            input_tokens in 0u32..1_000_000,
            output_tokens in 0u32..1_000_000,
        ) {
            let resp = ApiResponse {
                model: "test-model".into(),
                content: vec![serde_json::json!({"type": "text", "text": text.clone()})],
                stop_reason: Some("end_turn".into()),
                usage: ApiUsage { input_tokens, output_tokens },
            };
            let result = parse_api_response(resp).unwrap();
            prop_assert_eq!(result.content, ResponseContent::Text(text));
            prop_assert_eq!(result.usage.input_tokens, input_tokens);
            prop_assert_eq!(result.usage.output_tokens, output_tokens);
        }
    }
}
